use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::methods::ModelRef;

// Covers: each typed report's wire form (the `type` tags, snake_case names,
// unit-suffixed fields, and absent optionals left out) round-trips as a
// `rho/session/report` answer, which clients such as rho-ui read by name.
// Owner: protocol wire contract.
#[test]
fn every_typed_report_round_trips_through_its_wire_form() {
    let cases = [
        (
            SessionReportData::Context(SessionContextReport {
                tokens: 12_000,
                basis: ContextBasis::ProviderCalibrated,
                window_tokens: Some(200_000),
                system_prompt: vec![
                    ContextPromptPart {
                        source: PromptSourceKind::Base,
                        path: None,
                        tokens: 3_000,
                    },
                    ContextPromptPart {
                        source: PromptSourceKind::InstructionFile,
                        path: Some("/work/AGENTS.md".into()),
                        tokens: 500,
                    },
                ],
                tool_schemas: vec![
                    ContextToolSchemaPart {
                        origin: ToolSchemaOrigin::BuiltIn,
                        schemas: 12,
                        tokens: 4_000,
                    },
                    ContextToolSchemaPart {
                        origin: ToolSchemaOrigin::Mcp {
                            server: "github".into(),
                        },
                        schemas: 1,
                        tokens: 300,
                    },
                ],
                messages: vec![ContextMessagePart {
                    kind: ContextMessageKind::Assistant,
                    tokens: 2_000,
                }],
                tool_results: vec![
                    ContextToolResultPart {
                        tool: Some("read_file".into()),
                        tokens: 1_500,
                    },
                    ContextToolResultPart {
                        tool: None,
                        tokens: 100,
                    },
                ],
                request_context_tokens: 50,
            }),
            json!({
                "type": "context",
                "tokens": 12_000,
                "basis": "provider_calibrated",
                "window_tokens": 200_000,
                "system_prompt": [
                    {"source": "base", "tokens": 3_000},
                    {"source": "instruction_file", "path": "/work/AGENTS.md", "tokens": 500},
                ],
                "tool_schemas": [
                    {"origin": {"type": "built_in"}, "schemas": 12, "tokens": 4_000},
                    {"origin": {"type": "mcp", "server": "github"}, "schemas": 1, "tokens": 300},
                ],
                "messages": [{"kind": "assistant", "tokens": 2_000}],
                "tool_results": [{"tool": "read_file", "tokens": 1_500}, {"tokens": 100}],
                "request_context_tokens": 50,
            }),
        ),
        (
            SessionReportData::Hooks(SessionHooksReport {
                enabled: true,
                files: vec!["/home/me/.rho/hooks.toml".into()],
                skipped_untrusted: Some(SkippedHooksFile {
                    path: "/work/.rho/hooks.toml".into(),
                    error: None,
                }),
                invalid: Some(InvalidHooksFile {
                    path: "/home/me/.rho/hooks.toml".into(),
                    error: "missing field `on`".into(),
                }),
                hooks: vec![HookContract {
                    id: "user:fmt".into(),
                    status: HookStatus::Active,
                    event: HookEvent::AfterToolUse,
                    tools: HookTools::Matching {
                        patterns: vec!["str_replace".into(), "mcp__*".into()],
                    },
                    command: vec!["fmt".into(), "--all".into()],
                    working_directory: "/work".into(),
                    timeout_ms: 5_000,
                    environment: vec!["PATH".into()],
                    warnings: vec![HookWarning::MissingEditTools {
                        tools: vec!["apply_patch".into()],
                    }],
                }],
                recent_activity: vec![
                    HookActivityRecord {
                        hook: Some("user:fmt".into()),
                        event: HookEvent::BeforeToolUse,
                        outcome: HookOutcome::Denied {
                            reason: "no".into(),
                        },
                        duration_ms: Some(12),
                        output_truncated: false,
                    },
                    HookActivityRecord {
                        hook: None,
                        event: HookEvent::WorkflowNodeFinished,
                        outcome: HookOutcome::Dropped,
                        duration_ms: None,
                        output_truncated: false,
                    },
                ],
            }),
            json!({
                "type": "hooks",
                "enabled": true,
                "files": ["/home/me/.rho/hooks.toml"],
                "skipped_untrusted": {"path": "/work/.rho/hooks.toml"},
                "invalid": {"path": "/home/me/.rho/hooks.toml", "error": "missing field `on`"},
                "hooks": [{
                    "id": "user:fmt",
                    "status": "active",
                    "event": "after_tool_use",
                    "tools": {"type": "matching", "patterns": ["str_replace", "mcp__*"]},
                    "command": ["fmt", "--all"],
                    "working_directory": "/work",
                    "timeout_ms": 5_000,
                    "environment": ["PATH"],
                    "warnings": [{"type": "missing_edit_tools", "tools": ["apply_patch"]}],
                }],
                "recent_activity": [
                    {
                        "hook": "user:fmt",
                        "event": "before_tool_use",
                        "outcome": {"type": "denied", "reason": "no"},
                        "duration_ms": 12,
                        "output_truncated": false,
                    },
                    {
                        "event": "workflow_node_finished",
                        "outcome": {"type": "dropped"},
                        "output_truncated": false,
                    },
                ],
            }),
        ),
        (
            SessionReportData::Compaction(Box::new(SessionCompactionReport {
                auto_compact: true,
                context: CompactionContextSize {
                    tokens: 90_000,
                    local_tokens: 80_000,
                    provider_reported_tokens: Some(85_000),
                    provider_request_local_tokens: Some(75_000),
                },
                window_tokens: Some(200_000),
                threshold_tokens: Some(160_000),
                target_tokens: Some(60_000),
                local_target_tokens: Some(55_000),
                completed: CompletedCompactions {
                    count: 2,
                    removed_local_tokens: 120_000,
                    last: Some(CompactionSize {
                        before_local_tokens: 150_000,
                        after_local_tokens: 50_000,
                    }),
                },
                last_run: Some(CompactionRun {
                    trigger: CompactionRunTrigger::ContextOverflow,
                    outcome: CompactionRunOutcome::Completed,
                    tier: Some(CompactionTier::TextSummary),
                    summary_request: Some(SummaryRequestPath::SessionHistory),
                    model: Some(ModelRef {
                        provider: "anthropic".into(),
                        model: "claude".into(),
                    }),
                    elided_tool_results: 3,
                    context_tokens: 150_000,
                    prompt_tokens: Some(140_000),
                    output_tokens: Some(2_000),
                    cache_read_tokens: None,
                    cost_usd_micros: Some(41_000),
                    latency_ms: 9_500,
                    next_prompt_tokens: Some(52_000),
                    reread: Some(CompactionReread {
                        window_tool_calls: 24,
                        tool_calls: 10,
                        repeated: 1,
                        tracked: 4,
                    }),
                }),
                last_check: Some(CompactionCheck {
                    context_tokens: 90_000,
                    threshold: Some(CompactionThreshold::ContextTokens { tokens: 160_000 }),
                    skipped: Some(CompactionSkipReason::BelowThreshold),
                    extent: CompactionExtent::History,
                }),
            })),
            json!({
                "type": "compaction",
                "auto_compact": true,
                "context": {
                    "tokens": 90_000,
                    "local_tokens": 80_000,
                    "provider_reported_tokens": 85_000,
                    "provider_request_local_tokens": 75_000,
                },
                "window_tokens": 200_000,
                "threshold_tokens": 160_000,
                "target_tokens": 60_000,
                "local_target_tokens": 55_000,
                "completed": {
                    "count": 2,
                    "removed_local_tokens": 120_000,
                    "last": {"before_local_tokens": 150_000, "after_local_tokens": 50_000},
                },
                "last_run": {
                    "trigger": "context_overflow",
                    "outcome": "completed",
                    "tier": "text_summary",
                    "summary_request": "session_history",
                    "model": {"provider": "anthropic", "model": "claude"},
                    "elided_tool_results": 3,
                    "context_tokens": 150_000,
                    "prompt_tokens": 140_000,
                    "output_tokens": 2_000,
                    "cost_usd_micros": 41_000,
                    "latency_ms": 9_500,
                    "next_prompt_tokens": 52_000,
                    "reread": {"window_tool_calls": 24, "tool_calls": 10, "repeated": 1, "tracked": 4},
                },
                "last_check": {
                    "context_tokens": 90_000,
                    "threshold": {"type": "context_tokens", "tokens": 160_000},
                    "skipped": "below_threshold",
                    "extent": "history",
                },
            }),
        ),
        (
            SessionReportData::ProviderContext(SessionProviderContextReport {
                target: ModelRef {
                    provider: "openai".into(),
                    model: "gpt".into(),
                },
                omitted_blocks: 2,
                omitted_kinds: vec!["reasoning".into()],
                omitted_block_ids: vec![ProviderContextBlockId(7), ProviderContextBlockId(9)],
                source: Some(ModelRef {
                    provider: "anthropic".into(),
                    model: "claude".into(),
                }),
                can_compact: true,
                source_selectable: false,
            }),
            json!({
                "type": "provider_context",
                "target": {"provider": "openai", "model": "gpt"},
                "omitted_blocks": 2,
                "omitted_kinds": ["reasoning"],
                "omitted_block_ids": [7, 9],
                "source": {"provider": "anthropic", "model": "claude"},
                "can_compact": true,
                "source_selectable": false,
            }),
        ),
    ];
    for (typed, wire) in cases {
        let response = SessionReportResponse::new(typed);
        let expected = json!({"report": wire});
        assert_eq!(serde_json::to_value(&response).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<SessionReportResponse>(expected).unwrap(),
            response
        );
    }
}
