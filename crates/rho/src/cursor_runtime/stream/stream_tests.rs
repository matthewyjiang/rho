use pretty_assertions::assert_eq;

use rho_tools::tool_card::{ToolBody, ToolFact, ToolHeader, ToolStatus};

use crate::{run_artifacts::AttachmentEvent, subagent::RunState};

use super::*;

fn effects_from_fixture(name: &str) -> Vec<StreamEffect> {
    let body = match name {
        "live_text_thinking.ndjson" => include_str!("../fixtures/live_text_thinking.ndjson"),
        "live_edit.ndjson" => include_str!("../fixtures/live_edit.ndjson"),
        "live_shell_mid_snapshot.ndjson" => {
            include_str!("../fixtures/live_shell_mid_snapshot.ndjson")
        }
        "live_readonly_search.ndjson" => include_str!("../fixtures/live_readonly_search.ndjson"),
        other => panic!("unknown fixture {other}"),
    };
    let mut mapper = CursorStreamMapper::new();
    body.lines()
        .flat_map(|line| mapper.push_line(line))
        .collect()
}

/// Concatenated assistant text as the attachment reader would render it.
fn rendered_text(effects: &[StreamEffect]) -> String {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::AssistantTextDelta(text)) => Some(text),
            _ => None,
        })
        .cloned()
        .collect()
}

fn terminal(effects: &[StreamEffect]) -> &TerminalResult {
    effects
        .iter()
        .find_map(|effect| match effect {
            StreamEffect::Terminal(terminal) => Some(terminal),
            _ => None,
        })
        .expect("terminal result")
}

fn finished_cards(effects: &[StreamEffect]) -> Vec<&rho_tools::tool_card::ToolCard> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::ToolFinished {
                presentation: crate::presentation::Presentation::Card(card),
                ..
            }) => Some(card),
            _ => None,
        })
        .collect()
}

fn notices(effects: &[StreamEffect]) -> Vec<&str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StreamEffect::Attachment(AttachmentEvent::Notice(text)) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn final_snapshot_is_not_rendered_twice() {
    let effects = effects_from_fixture("live_text_thinking.ndjson");
    let terminal = terminal(&effects);
    // The terminal `result` text is the full concatenation Cursor computed;
    // rendered deltas must equal it exactly once.
    assert_eq!(
        rendered_text(&effects),
        terminal.result_text.clone().unwrap()
    );
    assert_eq!(notices(&effects), Vec::<&str>::new());
    assert!(effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Attachment(AttachmentEvent::ReasoningDelta(_))
    )));
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(
                effect,
                StreamEffect::Attachment(AttachmentEvent::StepStarted)
            ))
            .count(),
        1
    );
}

#[test]
fn mid_turn_snapshot_before_tool_call_is_dropped_and_segment_resets() {
    let effects = effects_from_fixture("live_shell_mid_snapshot.ndjson");
    let terminal = terminal(&effects);
    // Two text segments around one shell call. `result.result` joins them
    // with "\n"; the rendered stream has neither doubled.
    assert_eq!(
        rendered_text(&effects),
        terminal.result_text.clone().unwrap()
    );
    assert_eq!(notices(&effects), Vec::<&str>::new());

    let cards = finished_cards(&effects);
    assert_eq!(cards.len(), 1);
    let shell = cards[0];
    assert_eq!(shell.status, ToolStatus::Ok);
    assert_eq!(
        shell.header,
        ToolHeader::shell("$", Some("wc -l crates/rho/src/cli_runtime/*.rs".into()))
    );
    assert!(shell.facts.iter().any(|fact| matches!(
        fact,
        ToolFact::Exit {
            code: 0,
            duration_ms: Some(_)
        }
    )));
    assert!(
        matches!(&shell.body, ToolBody::Lines(lines) if lines.iter().any(|line| line.ends_with("child.rs")))
    );
}

#[test]
fn edit_result_renders_diff_from_diff_string() {
    let effects = effects_from_fixture("live_edit.ndjson");
    let cards = finished_cards(&effects);
    assert_eq!(cards.len(), 1);
    let edit = cards[0];
    assert_eq!(edit.status, ToolStatus::Ok);
    assert_eq!(
        edit.header,
        ToolHeader::call("Edit", Some("noforce.md".into()))
    );
    assert!(edit.facts.contains(&ToolFact::DiffStat {
        added: 1,
        removed: 0,
        path: None
    }));
    assert!(
        matches!(&edit.body, ToolBody::Diff(rows) if rows.iter().any(|row| row.text == "written without --force"))
    );
}

#[test]
fn read_only_search_cards_carry_exact_facts_from_fixture() {
    let effects = effects_from_fixture("live_readonly_search.ndjson");
    let cards = finished_cards(&effects);
    let summary = cards
        .iter()
        .map(|card| (card.header.clone(), card.status, card.facts.clone()))
        .collect::<Vec<_>>();
    let count = |label: &str, value: u64, detail: Option<&str>| ToolFact::Count {
        label: label.into(),
        value,
        detail: detail.map(str::to_string),
    };
    // Fixture order: grep, glob, then two reads; every card carries the
    // fact the wire result supports and nothing else.
    let grep = (
        ToolHeader::call("Grep", Some("allowlist, .".into())),
        ToolStatus::Ok,
        vec![count("matches", 0, None)],
    );
    let glob = (
        ToolHeader::call("Glob", Some("**/*".into())),
        ToolStatus::Ok,
        vec![count("files", 2, None)],
    );
    let read = |name: &str, lines: u64| {
        (
            ToolHeader::call("Read", Some(name.into())),
            ToolStatus::Ok,
            vec![count("lines", lines, None)],
        )
    };
    assert_eq!(
        summary,
        vec![
            grep,
            glob,
            read("noforce.md", 2),
            read("cursor-stream-protocol.md", 289),
        ]
    );
    let grep_card = &cards[0];
    assert_eq!(grep_card.match_pattern.as_deref(), Some("allowlist"));
    assert!(grep_card.match_case_sensitive);
    // Every started tool finished; no card was evicted or orphaned.
    let started = effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                StreamEffect::Attachment(AttachmentEvent::ToolStarted { .. })
            )
        })
        .count();
    assert_eq!(started, cards.len());
}

#[test]
fn init_and_result_populate_status_without_terminalizing() {
    let effects = effects_from_fixture("live_text_thinking.ndjson");
    let mut status = crate::subagent::RunStatus::default();
    for effect in &effects {
        if let StreamEffect::Status(patch) = effect {
            apply_status_patch(&mut status, patch.clone());
        }
    }
    assert_eq!(status.state, RunState::Running);
    assert_eq!(
        status.claude_session_id.as_deref(),
        Some("11111111-1111-4111-8111-111111111111")
    );
    assert_eq!(status.claude_model.as_deref(), Some("Composer 2.5"));
    // input = inputTokens + cacheReadTokens + cacheWriteTokens
    assert_eq!(status.input_tokens, Some(14414 + 5749));
    assert_eq!(status.output_tokens, Some(347));

    let terminal = terminal(&effects);
    assert!(matches!(
        terminal.classification,
        TerminalClassification::Success { .. }
    ));
    assert_eq!(terminal.num_turns, None);
    assert!(!effects.iter().any(|effect| matches!(
        effect,
        StreamEffect::Attachment(AttachmentEvent::Completed | AttachmentEvent::Failed(_))
    )));
}

#[test]
fn malformed_and_unknown_lines_become_notices() {
    let mut mapper = CursorStreamMapper::new();
    let cases: &[(&str, &str)] = &[
        ("{not json", "unparseable line"),
        (r#"{"type":"banana"}"#, "unknown frame banana"),
        (
            r#"{"type":"thinking","subtype":"exploded"}"#,
            "unknown frame thinking/exploded",
        ),
        (
            r#"{"type":"tool_call","subtype":"started"}"#,
            "missing call_id",
        ),
        (r#"[1,2]"#, "not a JSON object"),
    ];
    for (line, expected) in cases {
        let effects = mapper.push_line(line);
        let got = notices(&effects);
        assert_eq!(got.len(), 1, "{line}");
        assert!(got[0].contains(expected), "{line}: {}", got[0]);
    }
    assert_eq!(mapper.push_line("   "), Vec::new());
}

#[test]
fn non_success_tool_result_is_an_error_card() {
    let mut mapper = CursorStreamMapper::new();
    let started = r#"{"type":"tool_call","subtype":"started","call_id":"t1","tool_call":{"shellToolCall":{"args":{"command":"false"}}}}"#;
    let completed = r#"{"type":"tool_call","subtype":"completed","call_id":"t1","tool_call":{"shellToolCall":{"args":{"command":"false"},"result":{"rejected":{"message":"command not allowed"}}}}}"#;
    mapper.push_line(started);
    let effects = mapper.push_line(completed);
    let cards = finished_cards(&effects);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].status, ToolStatus::Error);
    assert!(cards[0].facts.contains(&ToolFact::Error {
        text: "command not allowed".into()
    }));
}

#[test]
fn snapshot_shaped_frame_with_new_text_is_rendered_with_notice() {
    let mut mapper = CursorStreamMapper::new();
    mapper.push_line(
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"abc"}]},"timestamp_ms":1}"#,
    );
    // Final-shaped frame (no timestamp, no model_call_id) whose text is not
    // the accumulated segment. Drift: keep the text, say so.
    let effects = mapper
        .push_line(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"xyz"}]}}"#);
    assert_eq!(rendered_text(&effects), "xyz");
    assert_eq!(notices(&effects).len(), 1);
}

fn delta(text: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":{}}}]}},"timestamp_ms":1}}"#,
        serde_json::to_string(text).unwrap()
    )
}

fn mid_snapshot(text: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":{}}}]}},"timestamp_ms":1,"model_call_id":"m1"}}"#,
        serde_json::to_string(text).unwrap()
    )
}

fn final_snapshot(text: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":{}}}]}}}}"#,
        serde_json::to_string(text).unwrap()
    )
}

const TOOL_STARTED: &str = r#"{"type":"tool_call","subtype":"started","call_id":"t1","tool_call":{"readToolCall":{"args":{"path":"a"}}}}"#;

// Covers: the snapshot rule keys on frame shape, so identical consecutive
// deltas are not mistaken for a replay, and a replay that lands after the
// tool call it precedes on the wire is still dropped.
// Owner: cursor stream mapper snapshot dedup
#[test]
fn snapshot_dedup_is_by_shape_then_content() {
    let cases: Vec<(&str, Vec<String>, &str, usize)> = vec![
        (
            "identical deltas then final snapshot",
            vec![delta("ab"), delta("ab"), final_snapshot("abab")],
            "abab",
            0,
        ),
        (
            "dots",
            vec![delta("."), delta("."), delta("."), final_snapshot("...")],
            "...",
            0,
        ),
        (
            "mid snapshot before tool",
            vec![
                delta("x"),
                delta("y"),
                mid_snapshot("xy"),
                TOOL_STARTED.into(),
                delta("z"),
                final_snapshot("z"),
            ],
            "xyz",
            0,
        ),
        (
            "mid snapshot after tool",
            vec![
                delta("x"),
                delta("y"),
                TOOL_STARTED.into(),
                mid_snapshot("xy"),
                delta("z"),
                final_snapshot("z"),
            ],
            "xyz",
            0,
        ),
        (
            "snapshot with unseen text is rendered with one notice",
            vec![delta("abc"), final_snapshot("xyz")],
            "abcxyz",
            1,
        ),
    ];
    for (name, lines, expected_text, expected_notices) in cases {
        let mut mapper = CursorStreamMapper::new();
        let effects = lines
            .iter()
            .flat_map(|line| mapper.push_line(line))
            .collect::<Vec<_>>();
        assert_eq!(rendered_text(&effects), expected_text, "{name}");
        assert_eq!(notices(&effects).len(), expected_notices, "{name}");
    }
}

// Covers: `success: null` and a shell that exited nonzero are error cards.
// Owner: cursor tool cards result outcome
#[test]
fn null_success_and_nonzero_shell_exit_are_error_cards() {
    let cases: &[(&str, &str, ToolStatus)] = &[
        (
            "success null",
            r#"{"type":"tool_call","subtype":"completed","call_id":"t1","tool_call":{"readToolCall":{"args":{"path":"a"},"result":{"success":null}}}}"#,
            ToolStatus::Error,
        ),
        (
            "shell nonzero exit",
            r#"{"type":"tool_call","subtype":"completed","call_id":"t1","tool_call":{"shellToolCall":{"args":{"command":"false"},"result":{"success":{"exitCode":1,"stdout":"","stderr":"boom"}}}}}"#,
            ToolStatus::Error,
        ),
        (
            "shell zero exit",
            r#"{"type":"tool_call","subtype":"completed","call_id":"t1","tool_call":{"shellToolCall":{"args":{"command":"true"},"result":{"success":{"exitCode":0,"stdout":"ok","stderr":""}}}}}"#,
            ToolStatus::Ok,
        ),
    ];
    for (name, line, expected) in cases {
        let mut mapper = CursorStreamMapper::new();
        let effects = mapper.push_line(line);
        let cards = finished_cards(&effects);
        assert_eq!(cards.len(), 1, "{name}");
        assert_eq!(cards[0].status, *expected, "{name}");
    }
}
