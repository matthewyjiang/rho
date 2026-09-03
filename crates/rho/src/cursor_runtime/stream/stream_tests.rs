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
            StreamEffect::Attachment(AttachmentEvent::ToolFinished { card, .. }) => Some(card),
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
    let expected = terminal.result_text.clone().unwrap();
    let rendered = rendered_text(&effects);
    assert_eq!(rendered.replace('\n', ""), expected.replace('\n', ""));
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
fn read_only_search_cards_carry_counts_and_grep_matches() {
    let effects = effects_from_fixture("live_readonly_search.ndjson");
    let cards = finished_cards(&effects);
    let by_verb = |verb: &str| {
        cards
            .iter()
            .filter(|card| matches!(&card.header, ToolHeader::Call { verb: v, .. } if v == verb))
            .collect::<Vec<_>>()
    };
    let globs = by_verb("Glob");
    assert!(!globs.is_empty());
    assert!(globs[0].facts.iter().any(
        |fact| matches!(fact, ToolFact::Count { label, .. } if label == "file" || label == "files")
    ));

    let greps = by_verb("Grep");
    assert!(!greps.is_empty());
    let grep = greps[0];
    assert!(grep.match_pattern.is_some());
    assert!(grep
        .facts
        .iter()
        .any(|fact| matches!(fact, ToolFact::Count { label, .. } if label.starts_with("match"))));

    let reads = by_verb("Read");
    assert!(!reads.is_empty());
    assert!(reads[0]
        .facts
        .iter()
        .any(|fact| matches!(fact, ToolFact::Count { label, .. } if label == "lines")));
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
    assert_eq!(terminal.num_turns, Some(1));
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
