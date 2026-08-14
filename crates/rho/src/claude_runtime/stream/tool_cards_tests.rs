use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::json;

use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolFact, ToolFamily, ToolHeader, ToolStatus,
};

use super::{finished_card, started_card, StartedClaudeTool, MAX_TOOL_PAYLOAD_CHARS};

fn tool(name: &str, input: serde_json::Value) -> StartedClaudeTool {
    StartedClaudeTool::from_name_input(Some(name), Some(&input))
}

// Covers: finished cards keep Claude verbs and native family/header dialects
// Owner: claude stream tool card mapper
#[test]
fn finished_cards_use_claude_names_and_native_dialects() {
    let cwd = Path::new("/tmp/ws");
    let cases = [
        (
            "Bash",
            json!({"command": "ls -la"}),
            None,
            "ok",
            ToolFamily::FileCommand,
            ToolHeader::shell("$", Some("ls -la".into())),
        ),
        (
            "Read",
            json!({"file_path": "/tmp/ws/note.txt"}),
            None,
            "1\thello",
            ToolFamily::FileCommand,
            ToolHeader::call("Read", Some("note.txt".into())),
        ),
        (
            "Glob",
            json!({"pattern": "*.md"}),
            None,
            "docs/a.md",
            ToolFamily::FileCommand,
            ToolHeader::call("Glob", Some("*.md".into())),
        ),
        (
            "Grep",
            json!({"pattern": "TODO", "path": "/tmp/ws/src"}),
            None,
            "src/lib.rs:1:TODO",
            ToolFamily::FileCommand,
            ToolHeader::call("Grep", Some("TODO, src".into())),
        ),
        (
            "Edit",
            json!({"file_path": "/tmp/ws/a.rs"}),
            None,
            "updated",
            ToolFamily::FileDiff,
            ToolHeader::call("Edit", Some("a.rs".into())),
        ),
        (
            "WebSearch",
            json!({"query": "rho tui"}),
            None,
            "hit",
            ToolFamily::Web,
            ToolHeader::call("WebSearch", Some("\"rho tui\"".into())),
        ),
        (
            "mcp__srv__list",
            json!({}),
            None,
            "items",
            ToolFamily::Default,
            ToolHeader::call("mcp__srv__list", None),
        ),
    ];

    for (name, input, result, content, family, header) in cases {
        let card = finished_card(
            Some(&tool(name, input)),
            /*ok*/ true,
            content,
            result.as_ref(),
            Some(cwd),
        );
        assert_eq!(card.family, family, "{name} family");
        assert_eq!(card.header, header, "{name} header");
        assert_eq!(card.status, ToolStatus::Ok, "{name} status");
    }
}

// Covers: structured Claude results become facts/diff bodies, not raw dumps
// Owner: claude stream tool card mapper
#[test]
fn finished_cards_use_structured_results() {
    let read = finished_card(
        Some(&tool("Read", json!({"file_path": "note.txt"}))),
        /*ok*/ true,
        "1\thello\n2\t",
        Some(&json!({"type": "text", "file": {"numLines": 2}})),
        None,
    );
    assert_eq!(
        read.facts,
        vec![ToolFact::Count {
            label: "lines".into(),
            value: 2,
            detail: None,
        }]
    );
    assert_eq!(read.body, ToolBody::None);

    let glob = finished_card(
        Some(&tool("Glob", json!({"pattern": "*.md"}))),
        /*ok*/ true,
        "Found 2 files",
        Some(&json!({"numFiles": 2, "filenames": ["a.md", "b.md"]})),
        None,
    );
    assert_eq!(
        glob.facts,
        vec![ToolFact::Count {
            label: "files".into(),
            value: 2,
            detail: None,
        }]
    );
    assert_eq!(
        glob.body,
        ToolBody::Lines(vec!["a.md".into(), "b.md".into()])
    );

    let edit = finished_card(
        Some(&tool("Edit", json!({"file_path": "a.rs"}))),
        /*ok*/ true,
        "updated",
        Some(&json!({
            "structuredPatch": [{
                "oldStart": 1,
                "newStart": 1,
                "oldLines": 1,
                "newLines": 1,
                "lines": ["-old", "+new"]
            }]
        })),
        None,
    );
    assert!(matches!(
        edit.facts.first(),
        Some(ToolFact::DiffStat {
            added: 1,
            removed: 1,
            ..
        })
    ));
    assert_eq!(
        edit.body,
        ToolBody::Diff(vec![
            DiffRow::new(DiffRowKind::Removed, Some(1), "old"),
            DiffRow::new(DiffRowKind::Added, Some(1), "new"),
        ])
    );
}

// Covers: failed results keep the Claude name and surface an error fact
// Owner: claude stream tool card mapper
#[test]
fn error_result_keeps_tool_name() {
    let card = finished_card(
        Some(&tool("Read", json!({"path": "missing.txt"}))),
        /*ok*/ false,
        "ENOENT: missing.txt",
        None,
        None,
    );
    assert_eq!(
        card.header,
        ToolHeader::call("Read", Some("missing.txt".into()))
    );
    assert_eq!(card.status, ToolStatus::Error);
    assert_eq!(
        card.facts,
        vec![ToolFact::Error {
            text: "ENOENT: missing.txt".into(),
        }]
    );
}

// Covers: finish without a matching start must not revive "tool result"
// Owner: claude stream tool card mapper
#[test]
fn finish_without_start_uses_generic_tool_verb() {
    let card = finished_card(None, /*ok*/ true, "ok", None, None);
    assert_eq!(card.header, ToolHeader::call("tool", None));
    assert_eq!(card.body, ToolBody::Lines(vec!["ok".into()]));
}

// Covers: running Bash uses the shell dialect, not Call(toolu_…)
// Owner: claude stream tool card mapper
#[test]
fn started_bash_card_uses_shell_header() {
    let card = started_card(&tool("Bash", json!({"command": "git status"})), None);
    assert_eq!(card.status, ToolStatus::Running);
    assert_eq!(
        card.header,
        ToolHeader::shell("$", Some("git status".into()))
    );
    assert_eq!(card.family, ToolFamily::FileCommand);
}

// Covers: finished bodies keep more than the collapsed 10-line budget
// Owner: claude stream tool card mapper
#[test]
fn finished_bash_body_keeps_depth_past_collapsed_budget() {
    let cases = [(40, 40, false), (60, 51, true)];
    for (input_lines, expected_len, truncated) in cases {
        let content = (0..input_lines)
            .map(|i| format!("out-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let card = finished_card(
            Some(&tool(
                "Bash",
                json!({"command": format!("seq {input_lines}")}),
            )),
            /*ok*/ true,
            &content,
            None,
            None,
        );
        let ToolBody::Lines(lines) = card.body else {
            panic!("{input_lines}-line payload should become a line body");
        };
        assert_eq!(lines.len(), expected_len, "{input_lines} lines");
        assert_eq!(lines[0], "out-0");
        if truncated {
            assert_eq!(lines[49], "out-49");
            assert_ne!(lines.last().map(String::as_str), Some("out-59"));
        } else {
            assert_eq!(lines.last().map(String::as_str), Some("out-39"));
        }
    }
}

// Covers: empty `{}` start input is treated as missing; later snapshots
// fill only when still empty; json fragments assemble into input
// Owner: claude stream tool card mapper
#[test]
fn apply_input_upgrades_empty_start() {
    let mut started = StartedClaudeTool::from_name_input(Some("Read"), Some(&json!({})));
    assert_eq!(started.input, None);
    assert!(started.apply_input(Some(&json!({"file_path": "a.rs"}))));
    assert_eq!(started.input, Some(json!({"file_path": "a.rs"})));
    assert!(!started.apply_input(Some(&json!({"file_path": "b.rs"}))));
    assert_eq!(started.input, Some(json!({"file_path": "a.rs"})));
}

// Covers: input_json_delta fragments become parsed input once complete
// Owner: claude stream tool card mapper
#[test]
fn push_input_json_assembles_fragments() {
    let mut started = StartedClaudeTool::from_name_input(Some("Read"), Some(&json!({})));
    assert!(!started.push_input_json("{\"file_path\":"));
    assert_eq!(started.input, None);
    assert!(started.push_input_json("\"note.txt\"}"));
    assert_eq!(started.input, Some(json!({"file_path": "note.txt"})));
    assert!(!started.apply_input(Some(&json!({"file_path": "other.txt"}))));
    assert_eq!(started.input, Some(json!({"file_path": "note.txt"})));
}

// Covers: oversized streamed Write fragments must still enrich the running path
// Owner: claude stream tool card mapper
#[test]
fn oversized_streamed_write_fragments_keep_running_path() {
    let content = "x".repeat(MAX_TOOL_PAYLOAD_CHARS + 64);
    let raw = serde_json::to_string(&json!({
        "file_path": "/tmp/ws/big.rs",
        "content": content
    }))
    .expect("serialize write input");
    assert!(raw.len() > MAX_TOOL_PAYLOAD_CHARS);
    let mut started = StartedClaudeTool::from_name_input(Some("Write"), Some(&json!({})));
    for chunk in raw.as_bytes().chunks(128) {
        let fragment = std::str::from_utf8(chunk).expect("ascii json chunk");
        started.push_input_json(fragment);
    }
    assert_eq!(
        started
            .input
            .as_ref()
            .and_then(|value| value.get("file_path")),
        Some(&json!("/tmp/ws/big.rs"))
    );
    let card = started_card(&started, Some(Path::new("/tmp/ws")));
    assert_eq!(
        card.header,
        ToolHeader::call("Write", Some("big.rs".into()))
    );
}

// Covers: a fragment over the raw assembly cap must not retain unbounded json
// Owner: claude stream tool card mapper
#[test]
fn push_input_json_caps_retained_fragments_and_keeps_path() {
    let content = "x".repeat(super::MAX_INPUT_JSON_CHARS + 64);
    let raw = format!(r#"{{"file_path":"/tmp/ws/huge.rs","content":"{content}"}}"#);
    assert!(raw.len() > super::MAX_INPUT_JSON_CHARS);
    let cases = [raw.len(), 1024];
    for chunk_size in cases {
        let mut started = StartedClaudeTool::from_name_input(Some("Write"), Some(&json!({})));
        if chunk_size >= raw.len() {
            started.push_input_json(&raw);
        } else {
            for chunk in raw.as_bytes().chunks(chunk_size) {
                let fragment = std::str::from_utf8(chunk).expect("ascii json chunk");
                started.push_input_json(fragment);
            }
        }
        assert!(
            started.input_json.len() <= super::MAX_INPUT_JSON_CHARS,
            "chunk_size {chunk_size}"
        );
        assert_eq!(
            started
                .input
                .as_ref()
                .and_then(|value| value.get("file_path")),
            Some(&json!("/tmp/ws/huge.rs")),
            "chunk_size {chunk_size}"
        );
        let card = started_card(&started, Some(Path::new("/tmp/ws")));
        assert_eq!(
            card.header,
            ToolHeader::call("Write", Some("huge.rs".into())),
            "chunk_size {chunk_size}"
        );
    }
}

// Covers: oversized Write content must not drop file_path from the card
// Owner: claude stream tool card mapper
#[test]
fn oversized_write_input_keeps_path_for_card() {
    let content = "x".repeat(MAX_TOOL_PAYLOAD_CHARS + 64);
    let started = tool(
        "Write",
        json!({"file_path": "/tmp/ws/big.rs", "content": content}),
    );
    assert_eq!(
        started
            .input
            .as_ref()
            .and_then(|value| value.get("file_path")),
        Some(&json!("/tmp/ws/big.rs"))
    );
    let card = finished_card(
        Some(&started),
        /*ok*/ true,
        "ok",
        Some(&json!({"type": "create"})),
        Some(Path::new("/tmp/ws")),
    );
    assert_eq!(
        card.header,
        ToolHeader::call("Write", Some("big.rs".into()))
    );
    assert!(card.body.is_diff());
    assert!(matches!(
        card.facts.first(),
        Some(ToolFact::DiffStat {
            added,
            removed: 0,
            ..
        }) if *added > 0
    ));
}

// Covers: escape-heavy oversized Write content must keep path and a fitting diff
// Owner: claude stream tool card mapper
#[test]
fn oversized_escaped_write_input_keeps_path_and_diff() {
    let content = "\"é\\\n".repeat(MAX_TOOL_PAYLOAD_CHARS / 2 + 32);
    let started = tool(
        "Write",
        json!({"file_path": "/tmp/ws/esc.rs", "content": content}),
    );
    let input = started.input.as_ref().expect("bounded write input");
    assert_eq!(input.get("file_path"), Some(&json!("/tmp/ws/esc.rs")));
    let bounded = input
        .get("content")
        .and_then(serde_json::Value::as_str)
        .expect("bounded write content");
    assert!(!bounded.is_empty());
    let encoded = serde_json::to_string(input).expect("encode bounded input");
    assert!(encoded.len() <= MAX_TOOL_PAYLOAD_CHARS);
    assert!(encoded.len() > MAX_TOOL_PAYLOAD_CHARS.saturating_sub(24));
    let card = finished_card(
        Some(&started),
        /*ok*/ true,
        "ok",
        Some(&json!({"type": "create"})),
        Some(Path::new("/tmp/ws")),
    );
    assert_eq!(
        card.header,
        ToolHeader::call("Write", Some("esc.rs".into()))
    );
    assert!(card.body.is_diff());
}

// Covers: unenriched Write must not be painted as a /dev/null create
// Owner: claude stream tool card mapper
#[test]
fn unenriched_write_is_not_painted_as_create() {
    let cases = [
        (None, false),
        (Some(json!({"type": "create", "structuredPatch": []})), true),
    ];
    for (result, expect_create_diff) in cases {
        let card = finished_card(
            Some(&tool(
                "Write",
                json!({"file_path": "note.txt", "content": "hello\nworld"}),
            )),
            /*ok*/ true,
            "ok",
            result.as_ref(),
            None,
        );
        assert_eq!(
            card.header,
            ToolHeader::call("Write", Some("note.txt".into()))
        );
        if expect_create_diff {
            assert!(card.body.is_diff());
            assert!(matches!(
                card.facts.first(),
                Some(ToolFact::DiffStat {
                    added,
                    removed: 0,
                    ..
                }) if *added > 0
            ));
        } else {
            assert!(!card.body.is_diff());
            assert!(!matches!(
                card.facts.first(),
                Some(ToolFact::DiffStat {
                    added,
                    removed: 0,
                    ..
                }) if *added > 0
            ));
        }
    }
}

// Covers: patchless Write update must not be painted as a new-file create
// Owner: claude stream tool card mapper
#[test]
fn patchless_write_update_is_not_painted_as_create() {
    let cases = [
        (
            json!({
                "type": "update",
                "content": "beta",
                "structuredPatch": [],
                "originalFile": "alpha"
            }),
            Some((1_u64, 1_u64)),
        ),
        (
            json!({
                "type": "update",
                "content": "beta",
                "structuredPatch": []
            }),
            None,
        ),
    ];
    for (result, expected_stat) in cases {
        let card = finished_card(
            Some(&tool(
                "Write",
                json!({"file_path": "note.txt", "content": "beta"}),
            )),
            /*ok*/ true,
            "updated",
            Some(&result),
            None,
        );
        match expected_stat {
            Some((added, removed)) => {
                assert_eq!(
                    card.facts.first(),
                    Some(&ToolFact::DiffStat {
                        added,
                        removed,
                        path: None,
                    })
                );
                assert!(card.body.is_diff());
            }
            None => {
                assert!(!matches!(
                    card.facts.first(),
                    Some(ToolFact::DiffStat {
                        added,
                        removed: 0,
                        ..
                    }) if *added > 0
                ));
                assert!(!card.body.is_diff());
            }
        }
    }
}
