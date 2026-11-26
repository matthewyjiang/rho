use std::collections::HashMap;

use pretty_assertions::assert_eq;
use rho_sdk::{
    model::{
        ContentBlock, ModelEvent, ModelIdentity, ModelResponse, ModelUsage, ToolCall, ToolSpec,
    },
    provider::{ScriptedProvider, ScriptedTurn},
    tool::{ScriptedTool, ScriptedToolOutcome, ToolOutput},
    Rho, SessionOptions, SystemPrompt,
};

use rho_protocol::{
    ContextBasis, ContextMessageKind, PromptSourceKind as WirePromptSource, ToolSchemaOrigin,
};

use super::{largest_remainder, session_context_report, ContextReport};
use crate::prompt::{PromptSource, PromptSourceKind};

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("{name} tool"),
        input_schema: serde_json::json!({"type": "object"}),
    }
}

/// Session whose provider reports `reported` prompt tokens after a
/// `read_file` call, with one built-in and one MCP tool advertised.
async fn calibrated_session(reported: u64) -> rho_sdk::Session {
    let provider = ScriptedProvider::new(
        ModelIdentity::new("test", "test", "test"),
        [
            ScriptedTurn::completed(ModelResponse::Assistant(vec![ContentBlock::ToolCall(
                ToolCall::new(
                    "read-1".into(),
                    "read_file".into(),
                    serde_json::json!({"path": "src/main.rs"}),
                ),
            )])),
            ScriptedTurn::streaming(
                vec![ModelEvent::Usage(ModelUsage {
                    input_tokens: Some(reported),
                    ..ModelUsage::default()
                })],
                ModelResponse::Assistant(vec![ContentBlock::Text("done".into())]),
            ),
        ],
    );
    let runtime = Rho::builder()
        .provider(provider)
        .system_prompt(SystemPrompt::Custom("x".repeat(4_000)))
        .tool(ScriptedTool::new(
            spec("read_file"),
            ScriptedToolOutcome::Success(ToolOutput::text("y".repeat(2_000))),
        ))
        .tool(ScriptedTool::new(
            spec("mcp__mem__query"),
            ScriptedToolOutcome::Success(ToolOutput::text("unused")),
        ))
        .build()
        .unwrap();
    let session = runtime.session(SessionOptions::new()).await.unwrap();
    session.complete("read main").await.unwrap();
    session
}

// Covers: sources misattributed (MCP schemas counted as built-in, tool output
// not grouped by tool, prompt split ignoring source bytes) and parts left on
// the local scale while the total is the provider-calibrated one.
// Owner: Rho's /context grouping policy over the SDK itemization.
#[tokio::test]
async fn report_groups_sources_and_scales_parts_to_calibrated_total() {
    let session = calibrated_session(100_000).await;
    let sources = [
        PromptSource {
            kind: PromptSourceKind::Base,
            path: None,
            bytes: 3_000,
        },
        PromptSource {
            kind: PromptSourceKind::Agents,
            path: Some("/repo/AGENTS.md".into()),
            bytes: 1_000,
        },
    ];
    let mcp_tools = HashMap::from([("mcp__mem__query".to_string(), "mem".to_string())]);

    let report = session_context_report(
        &session.context_breakdown(),
        &sources,
        &mcp_tools,
        Some(200_000),
    );

    let layout = (
        report
            .system_prompt
            .iter()
            .map(|part| (part.source, part.path.clone()))
            .collect::<Vec<_>>(),
        report
            .tool_schemas
            .iter()
            .map(|part| (part.origin.clone(), part.schemas))
            .collect::<Vec<_>>(),
        report
            .messages
            .iter()
            .map(|part| part.kind)
            .collect::<Vec<_>>(),
        report
            .tool_results
            .iter()
            .map(|part| part.tool.clone())
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        layout,
        (
            vec![
                (WirePromptSource::Base, None),
                (
                    WirePromptSource::InstructionFile,
                    Some("/repo/AGENTS.md".into())
                ),
            ],
            vec![
                (
                    ToolSchemaOrigin::Mcp {
                        server: "mem".into()
                    },
                    1
                ),
                (ToolSchemaOrigin::BuiltIn, 1),
            ],
            vec![ContextMessageKind::Assistant, ContextMessageKind::User],
            vec![Some("read_file".to_owned())],
        )
    );
    // The overlay groups the parts the same way, in its order.
    let overlay = ContextReport::from(&report);
    let shown_layout: Vec<_> = overlay
        .groups
        .iter()
        .map(|group| {
            let rows: Vec<_> = group
                .rows
                .iter()
                .map(|row| (row.label.as_str(), row.detail.as_deref()))
                .collect();
            (group.label.as_str(), rows)
        })
        .collect();
    assert_eq!(
        shown_layout,
        vec![
            (
                "System prompt",
                vec![
                    ("Rho instructions", None),
                    ("Instruction file", Some("/repo/AGENTS.md")),
                ]
            ),
            (
                "Tool schemas",
                vec![
                    ("MCP mem", Some("1 schema")),
                    ("Built-in tools", Some("1 schema")),
                ]
            ),
            (
                "Messages",
                vec![
                    ("Assistant (text, reasoning, tool calls)", None),
                    ("User and host notes", None),
                ]
            ),
            ("Tool results", vec![("read_file", None)]),
        ]
    );
    assert_eq!(report.basis, ContextBasis::ProviderCalibrated);
    assert_eq!(report.tokens, session.context_estimate().tokens());

    let [base, agents] = [&report.system_prompt[0], &report.system_prompt[1]];
    assert!(base.tokens.abs_diff(3 * agents.tokens) <= 3, "{report:?}");

    // Parts are on the calibrated scale: they add up to the total, short
    // only of fixed request framing and per-part rounding.
    let shown: u64 = report
        .system_prompt
        .iter()
        .map(|part| part.tokens)
        .sum::<u64>()
        + report
            .tool_schemas
            .iter()
            .map(|part| part.tokens)
            .sum::<u64>()
        + report.messages.iter().map(|part| part.tokens).sum::<u64>()
        + report
            .tool_results
            .iter()
            .map(|part| part.tokens)
            .sum::<u64>()
        + report.request_context_tokens;
    assert!(
        report.tokens - shown < report.tokens / 100,
        "{shown} of {}",
        report.tokens
    );
}

// Covers: the system prompt split biasing one source (all leftover tokens to
// the largest floored share) or failing to sum to the measured message.
// Owner: /context system prompt attribution.
#[test]
fn largest_remainder_distributes_leftovers_by_fraction() {
    struct Case {
        name: &'static str,
        total: u64,
        weights: &'static [u64],
        expected: Option<&'static [u64]>,
    }
    let cases = [
        Case {
            name: "exact split",
            total: 8,
            weights: &[3, 1],
            expected: Some(&[6, 2]),
        },
        Case {
            name: "leftovers follow fractions",
            total: 8,
            weights: &[5, 3, 2],
            expected: Some(&[4, 2, 2]),
        },
        Case {
            name: "fewer tokens than sources",
            total: 2,
            weights: &[1, 1, 1],
            expected: Some(&[1, 1, 0]),
        },
        Case {
            name: "zero weight gets nothing",
            total: 5,
            weights: &[0, 1],
            expected: Some(&[0, 5]),
        },
        Case {
            name: "no weight to split by",
            total: 5,
            weights: &[0, 0],
            expected: None,
        },
    ];
    for case in cases {
        assert_eq!(
            largest_remainder(case.total, case.weights).as_deref(),
            case.expected,
            "{}",
            case.name
        );
    }
}
