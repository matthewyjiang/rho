use pretty_assertions::assert_eq;
use rho_tools::tool_card::{
    DiffRow, DiffRowKind, ToolBody, ToolFact, ToolFamily, ToolHeader, ToolStatus,
};

use super::*;
use crate::tui::tool_output_ui::tool_output_toggleable;

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn short_diff_card() -> ToolCard {
    ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::call("edit_file", Some("theme.rs".into())),
    )
    .with_facts(vec![ToolFact::DiffStat {
        added: 54,
        removed: 2,
        path: Some("theme.rs".into()),
    }])
    .with_body(ToolBody::Diff(vec![
        DiffRow::new(DiffRowKind::Removed, Some(41), "old"),
        DiffRow::new(DiffRowKind::Added, Some(41), "new"),
    ]))
}

#[test]
fn renders_edit_card_with_stat_child_and_diff_rows() {
    let card = short_diff_card();
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 4, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        rendered,
        vec![
            "✓ edit_file(theme.rs)".to_string(),
            "  ├ +54 -2 lines | theme.rs".to_string(),
            "    41 - old".to_string(),
            "    41 + new".to_string(),
        ]
    );
}

#[test]
fn short_diff_shows_its_body_while_collapsed() {
    let card = short_diff_card();
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 4, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(rendered.iter().any(|line| line.contains("41 - old")));
    assert!(rendered.iter().any(|line| line.contains("41 + new")));
    assert!(!rendered.iter().any(|line| line.contains("more lines")));
}

#[test]
fn diff_gutter_pads_line_numbers_to_a_common_width() {
    let card = short_diff_card().with_body(ToolBody::Diff(vec![
        DiffRow::new(DiffRowKind::Context, Some(9), "keep"),
        DiffRow::new(DiffRowKind::Added, Some(1204), "new"),
    ]));
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 4, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        rendered[2..],
        ["       9   keep".to_string(), "    1204 + new".to_string()]
    );
}

#[test]
fn unnumbered_diff_rows_render_without_a_gutter() {
    let card = short_diff_card().with_body(ToolBody::Diff(vec![
        DiffRow::new(DiffRowKind::File, None, "src/lib.rs"),
        DiffRow::new(DiffRowKind::Added, None, "new"),
    ]));
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 4, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        rendered[2..],
        ["    src/lib.rs".to_string(), "    + new".to_string()]
    );
}

#[test]
fn over_budget_diff_is_toggleable_via_display_plan() {
    let tool = ToolEntry {
        card: short_diff_card(),
        expanded: false,
        image: None,
    };
    assert!(
        tool_output_toggleable(&tool, 2, 80),
        "a diff past the budget must be expandable"
    );

    let mut expanded = tool.clone();
    expanded.expanded = true;
    assert!(
        tool_output_toggleable(&expanded, 2, 80),
        "expanded diffs must stay toggleable so users can collapse"
    );
}

#[test]
fn long_collapsed_diff_shows_expand_prompt() {
    let body = (0..12)
        .map(|index| {
            let kind = if index % 2 == 0 {
                DiffRowKind::Removed
            } else {
                DiffRowKind::Added
            };
            DiffRow::new(kind, Some(index + 1), format!("line {index}"))
        })
        .collect::<Vec<_>>();
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileDiff,
        ToolHeader::call("edit_file", Some("big.rs".into())),
    )
    .with_facts(vec![ToolFact::DiffStat {
        added: 6,
        removed: 6,
        path: Some("big.rs".into()),
    }])
    .with_body(ToolBody::Diff(body));

    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 4, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("more lines, ctrl+o to expand")),
        "long collapsed diff should expose hidden body count: {rendered:?}"
    );
}

#[test]
fn many_facts_truncate_under_shared_budget() {
    let facts = (0..6)
        .map(|index| ToolFact::Meta {
            text: format!("fact-{index}"),
        })
        .collect::<Vec<_>>();
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("tool", Some("target".into())),
    )
    .with_facts(facts)
    .with_body(ToolBody::Lines(vec!["body-a".into(), "body-b".into()]));

    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 2, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(rendered.iter().any(|line| line.contains("fact-0")));
    assert!(rendered.iter().any(|line| line.contains("fact-1")));
    assert!(!rendered.iter().any(|line| line.contains("fact-2")));
    assert!(!rendered.iter().any(|line| line.contains("body-a")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("... 6 more lines, ctrl+o to expand")),
        "shared budget should fold remaining facts and body: {rendered:?}"
    );
}

#[test]
fn header_and_facts_share_tiny_body_budget() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some("cargo test".into())),
    )
    .with_facts(vec![
        ToolFact::Meta {
            text: "timeout 30s".into(),
        },
        ToolFact::Exit {
            code: 0,
            duration_ms: Some(100),
        },
    ])
    .with_body(ToolBody::Lines(vec![
        "line1".into(),
        "line2".into(),
        "line3".into(),
    ]));

    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 1, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(rendered[0].starts_with("✓ $ cargo test"));
    assert!(rendered.iter().any(|line| line.contains("timeout 30s")));
    assert!(
        !rendered.iter().any(|line| line.contains("exit 0")),
        "second fact must yield to the shared budget: {rendered:?}"
    );
    assert!(!rendered.iter().any(|line| line.contains("line1")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("... 4 more lines, ctrl+o to expand")),
        "tiny budget should report remaining facts+body: {rendered:?}"
    );
}

#[test]
fn long_wrapped_fact_counts_against_terminal_row_budget() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::Default,
        ToolHeader::call("tool", Some("target".into())),
    )
    .with_facts(vec![ToolFact::Text {
        text: "word ".repeat(80),
    }]);
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 40, 1, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert_eq!(
        rendered
            .iter()
            .filter(|line| line.contains("word") || line.contains("more lines"))
            .count(),
        2,
        "one terminal row of content plus expand cue: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("more lines, ctrl+o to expand")),
        "wrapped overflow must stay expandable: {rendered:?}"
    );
    assert!(card_is_toggleable(&card, 40, 1, /*expanded*/ false));
}

#[test]
fn tool_entry_lines_use_trailing_blank_only() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::call("read_file", Some("main.rs".into())),
    );
    let tool = crate::tui::ToolEntry {
        card,
        expanded: false,
        image: None,
    };
    let lines = tool_entry_lines(&tool, 40, 4);
    assert!(
        line_text(&lines[0]).contains("✓ read_file(main.rs)"),
        "unexpected header: {}",
        line_text(&lines[0])
    );
    assert!(
        line_text(lines.last().expect("card lines")).is_empty(),
        "expected a single trailing spacer"
    );
    assert!(
        lines.len() >= 2 && !line_text(&lines[lines.len() - 2]).is_empty(),
        "tool cards should not keep a leading spacer blank"
    );
}

#[test]
fn long_shell_header_wraps_command_under_prompt() {
    let command = "cargo test -p rho-coding-agent --lib interactive_presenter -- --nocapture";
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some(command.into())),
    )
    .with_facts(vec![ToolFact::Meta {
        text: "timeout 30s".into(),
    }]);
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 40, 10, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        rendered.len() >= 3,
        "long command should wrap before facts: {rendered:?}"
    );
    assert!(
        rendered[0].starts_with("● $ "),
        "marker+prompt stay on first header row: {rendered:?}"
    );
    assert!(
        !rendered[0].contains('├') && !rendered[0].contains('└'),
        "header must not use tree glyphs: {rendered:?}"
    );
    // Continuation uses a tree-column stem, then hangs under the primary.
    let cont = &rendered[1];
    assert!(
        cont.contains('│'),
        "header continuation should draw a │ stem: {rendered:?}"
    );
    assert!(
        !cont.contains('├') && !cont.contains('└'),
        "header continuation must not use child branch glyphs: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("timeout 30s") && (line.contains('├') || line.contains('└'))),
        "facts keep tree structure after wrapped header: {rendered:?}"
    );
    let joined: String = rendered.iter().map(|line| line.trim()).collect();
    assert!(
        joined.contains("interactive_presenter") && joined.contains("nocapture"),
        "full command remains visible after wrap: {rendered:?}"
    );
}

#[test]
fn multiline_shell_header_preserves_command_line_breaks() {
    // Multi-line bash used to collapse: `\n` is zero-width and ratatui drops it,
    // so `check\ngit` rendered as `checkgit`. Hard-break then soft-wrap fixes that.
    let command = "\
cargo fmt --all -- --check
git add crates/rho-tools/src/bash.rs
git commit -m \"$(cat <<'EOF'
fix(tools): harden escaped-process bash timeout test

Flush and fsync the pid file.
EOF
)\"
git push
git status -sb
";
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some(command.into())),
    )
    .with_facts(vec![
        ToolFact::Meta {
            text: "timeout 60s".into(),
        },
        ToolFact::Exit {
            code: 0,
            duration_ms: Some(2600),
        },
    ]);
    let mut lines = Vec::new();
    // Wide enough that only hard newlines create header rows.
    push_tool_card(&mut lines, &card, 120, 20, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        rendered,
        vec![
            "✓ $ cargo fmt --all -- --check",
            "  │ git add crates/rho-tools/src/bash.rs",
            "  │ git commit -m \"$(cat <<'EOF'",
            "  │ fix(tools): harden escaped-process bash timeout test",
            "  │",
            "  │ Flush and fsync the pid file.",
            "  │ EOF",
            "  │ )\"",
            "  │ git push",
            "  │ git status -sb",
            "  ├ timeout 60s",
            "  └ exit 0 · 2.6s",
        ]
    );
}

#[test]
fn multiline_shell_header_keeps_soft_wrap_inside_long_line() {
    let command = "echo one\ncargo test -p rho-coding-agent --lib interactive_presenter -- --nocapture\necho two";
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some(command.into())),
    );
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 40, 10, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        rendered,
        vec![
            "● $ echo one",
            "  │ cargo test -p rho-coding-agent --lib",
            "  │ interactive_presenter --",
            "  │ --nocapture",
            "  │ echo two",
        ]
    );
}

#[test]
fn long_call_header_wraps_primary_inside_parens() {
    let card = ToolCard::new(
        ToolStatus::Ok,
        ToolFamily::FileCommand,
        ToolHeader::call(
            "read_file",
            Some("crates/rho/src/tui/tool_card_render.rs".into()),
        ),
    );
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 28, 4, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        rendered[0].starts_with("✓ read_file("),
        "verb and open paren stay on first row: {rendered:?}"
    );
    assert!(rendered.len() >= 2, "long path should wrap: {rendered:?}");
    assert!(
        rendered.iter().skip(1).all(|line| line.contains('│')),
        "wrapped call primary should use │ stems: {rendered:?}"
    );
    let path_text: String = rendered
        .iter()
        .map(|line| line.trim().trim_start_matches('│').trim().to_string())
        .collect();
    assert!(
        path_text.contains("tool_card_render.rs") && path_text.contains(')'),
        "path and closing paren remain visible: {rendered:?}"
    );
}

#[test]
fn running_shell_card_renders_streamed_stdout_body() {
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::FileCommand,
        ToolHeader::shell("$", Some("cargo test".into())),
    )
    .with_facts(vec![
        ToolFact::Meta {
            text: "timeout 30s".into(),
        },
        ToolFact::Meta {
            text: "running".into(),
        },
    ])
    .with_body(ToolBody::Lines(vec![
        "compiling rho".into(),
        "running 12 tests".into(),
    ]));
    let tool = crate::tui::ToolEntry {
        card,
        expanded: false,
        image: None,
    };
    let rendered: Vec<String> = tool_entry_lines(&tool, 60, 10)
        .iter()
        .map(line_text)
        .collect();
    assert!(
        rendered.iter().any(|line| line.contains("compiling rho")),
        "streamed stdout missing from rendered card: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("running 12 tests")),
        "streamed stdout missing from rendered card: {rendered:?}"
    );
}

#[test]
fn finished_background_agent_keeps_running_marker() {
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Agent,
        ToolHeader::status_first("worker", "running in background"),
    )
    .with_facts(vec![ToolFact::Text {
        text: "fixture stream".into(),
    }])
    .with_body(ToolBody::Lines(vec!["abc123 · rho attach abc123".into()]));
    let mut lines = Vec::new();
    push_tool_card(&mut lines, &card, 80, 10, /*expanded*/ false);
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    assert!(
        rendered[0].starts_with("● worker  running in background"),
        "background spawn must keep the running marker after tool finish: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("fixture stream")),
        "background task text missing: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("abc123 · rho attach abc123")),
        "background run meta missing: {rendered:?}"
    );
}
