use super::*;
use crate::tui::terminal_graph::{MAX_LINES, WRAP_WIDTH};
use pretty_assertions::assert_eq;
use ratatui::text::{Line, Span};

fn plain(lines: &[ratatui::text::Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn rendered(source: &str, width: usize) -> Vec<String> {
    match render_mermaid(source, width) {
        MermaidRender::Rendered(lines) => plain(&lines),
        MermaidRender::Clipped { hidden_columns, .. } => {
            panic!("unexpected clip for {source:?}: {hidden_columns} columns hidden")
        }
        MermaidRender::Fallback(reason) => {
            panic!("unexpected fallback for {source:?}: {reason:?}")
        }
    }
}

#[test]
fn renders_quality_supported_families_without_ansi_or_width_overflow() {
    let fixtures = [
        "flowchart LR\nA[Parse] --> B[Render]",
        "sequenceDiagram\nparticipant Alice\nparticipant Bob\nAlice->>Bob: Hello",
        "stateDiagram-v2\nReady --> Waiting",
        "erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\nstring name\n}",
        "classDiagram\nAnimal <|-- Duck\nclass Animal {\n+name: String\n+speak()\n}",
        "gitGraph\ncommit id: \"init\" msg: \"init\"\ncommit id: \"next\" msg: \"next\"",
        "gantt\ntitle Plan\nParser :p1, 2026-01-01, 3d",
        "mindmap\n  Root\n    Child",
    ];

    for source in fixtures {
        let lines = rendered(source, 240);
        assert!(!lines.is_empty(), "{source}");
        assert!(lines.iter().all(|line| !line.contains('\x1b')), "{source}");
        assert!(
            lines.iter().all(|line| display_width(line) <= 240),
            "{source}"
        );
    }
}

#[test]
fn unsupported_families_cleanly_fall_back() {
    for source in [
        "pie\n\"Dogs\" : 5",
        "journey\nsection Work\nCode: 5: Me",
        "timeline\n2025 : Shipped",
        "quadrantChart\nFast: [0.8, 0.8]",
        "sankey-beta\nInput,Output,1",
        "xychart-beta\nx-axis [a, b]",
        "block-beta\nA B",
        "architecture-beta\nservice api(server)[API]",
        "packet-beta\n0-15: \"Source\"",
        "requirementDiagram\nrequirement test",
        "C4Context\nPerson(user, \"User\")",
        "zenuml\nAlice->Bob: Hi",
        "kanban\nTodo[Todo]",
        "radar-beta\naxis A",
        "treemap-beta\nRoot",
    ] {
        assert_eq!(
            render_mermaid(source, 240),
            MermaidRender::Fallback(MermaidFallback::Unsupported),
            "{source}"
        );
    }
}

#[test]
fn applies_source_model_and_canvas_limits_before_or_after_painting() {
    assert_eq!(
        render_mermaid(&"x".repeat(MAX_SOURCE_BYTES + 1), 80),
        MermaidRender::Fallback(MermaidFallback::SourceBytes)
    );
    let too_many_lines = std::iter::repeat_n("%% comment", MAX_SOURCE_LINES + 1)
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        render_mermaid(&too_many_lines, 80),
        MermaidRender::Fallback(MermaidFallback::SourceLines)
    );
    let too_many_nodes = format!(
        "flowchart LR\n{}",
        (0..=MAX_PRIMARY_ENTITIES)
            .map(|index| format!("N{index}[node {index}]"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert_eq!(
        render_mermaid(&too_many_nodes, 240),
        MermaidRender::Fallback(MermaidFallback::StructuralLimit)
    );
    assert_eq!(
        render_mermaid("flowchart LR\nA[a label that cannot fit]", 4),
        MermaidRender::Fallback(MermaidFallback::TooWide)
    );
    // A grouped node label may fit the normal wrap but exceed the line budget
    // at narrower wraps. Fall back instead of truncating.
    let compaction_label = "x".repeat(WRAP_WIDTH * (MAX_LINES - 1));
    let compacted_group = format!("flowchart TD\nsubgraph Group\nA[{compaction_label}]\nend");
    assert_eq!(
        render_mermaid(&compacted_group, 20),
        MermaidRender::Fallback(MermaidFallback::TooWide)
    );
}

// Covers: live mermaid retries blank/malformed prefixes, keeps last-good art,
// and declines sticky failures so the open fence stays source until close.
// Owner: mermaid open prefix
#[test]
fn open_prefix_paints_valid_walks_back_malformed_and_declines_sticky() {
    assert!(matches!(
        render_open_prefix("flowchart LR\nA --> B", "flowchart LR\nA --> B", 80),
        Some(ClosedPanel::Art {
            title: "MERMAID",
            ..
        })
    ));
    let good = "flowchart LR\nA --> B";
    let malformed = format!("{good}\nA -->");
    let Some(ClosedPanel::Art { lines: kept, .. }) = render_open_prefix(&malformed, &malformed, 80)
    else {
        panic!("malformed last line should keep last-good art");
    };
    let MermaidRender::Rendered(expected) = render_mermaid(good, 80) else {
        panic!("control prefix should render");
    };
    assert_eq!(plain(&kept), plain(&expected));
    assert!(render_open_prefix("  \n", "  \n", 80).is_none());
    assert!(render_open_prefix("pie\n\"Dogs\" : 5", "pie\n\"Dogs\" : 5", 240).is_none());
    assert!(render_open_prefix(
        "flowchart LR\nclick A \"https://example.com\"",
        "flowchart LR\nclick A \"https://example.com\"",
        80
    )
    .is_none());
}

#[test]
fn rejects_blank_malformed_unsafe_and_link_bearing_sources() {
    assert_eq!(
        render_mermaid("  \n", 80),
        MermaidRender::Fallback(MermaidFallback::Blank)
    );
    assert_eq!(
        render_mermaid("unknownDiagram\nA", 80),
        MermaidRender::Fallback(MermaidFallback::Unsupported)
    );
    assert_eq!(
        render_mermaid("flowchart LR\nA -->", 80),
        MermaidRender::Fallback(MermaidFallback::Malformed)
    );
    for source in [
        "flowchart LR\nclick A \"https://example.com\"",
        "flowchart LR\nA[<script>alert(1)</script>]",
        "flowchart LR\nA[javascript:alert(1)]",
        "flowchart LR\nA[escape \u{1b}[31m]",
    ] {
        assert_eq!(
            render_mermaid(source, 80),
            MermaidRender::Fallback(MermaidFallback::UnsafeContent),
            "{source:?}"
        );
    }
}

// Covers: conversion must refuse a sequence with no participants
// Owner: mermaid model conversion
#[test]
fn empty_sequence_stays_unsupported() {
    assert_eq!(
        render_mermaid("sequenceDiagram", 240),
        MermaidRender::Fallback(MermaidFallback::Unsupported)
    );
}

// Covers: everyday extras must keep their approximated structure, not just paint
// Owner: mermaid model conversion
#[test]
fn paints_common_approximations() {
    let cases = [
        (
            "parallel_edges",
            "flowchart TD\nA -->|one| B\nA -->|two| B",
            &["one / two"][..],
        ),
        (
            "class_multiplicity",
            "classDiagram\nA \"1\" --> \"*\" B : owns",
            &["1 owns *"],
        ),
        (
            "cylinder_and_stadium",
            "flowchart TD\nA[(database)] --> B([api])",
            &["database", "api"],
        ),
        (
            "state_note",
            "stateDiagram-v2\n[*] --> Ready\nnote right of Ready: queued",
            &["Ready (note: queued)"],
        ),
        (
            "long_edge_label",
            "flowchart TD\nA -->|too many lines / cells / ANSI / wider than pane| B",
            &["too many lines"],
        ),
        (
            "long_group_title",
            "flowchart TD\nsubgraph abcdefghijklmnopqrstuvwxyz\nA[ok]\nend",
            &["abcdefghijklmnopqrstuvwxyz", "ok"],
        ),
    ];

    for (name, source, needles) in cases {
        let art = rendered(source, 80).join("\n");
        for needle in needles {
            assert!(art.contains(needle), "{name}: missing {needle:?} in\n{art}");
        }
    }
}

// Covers: a CJK grapheme straddling the clip boundary must not free a column
// for a later span
// Owner: mermaid clip math
#[test]
fn clip_line_drops_straddling_cjk_without_bleeding_later_spans() {
    let line = Line::from(vec![Span::raw("ab你"), Span::raw("later")]);
    let clipped = clip_line_to_width(line, 3);
    let text = plain(&[clipped]).join("");
    assert_eq!(text, "ab");
    assert!(display_width(&text) <= 3);
}

// Covers: a diagram wider than every compaction rung must clip, not dump source
// Owner: mermaid render fallback policy
#[test]
fn clips_oversized_diagram_instead_of_source_fallback() {
    // Wide by construction: one rank of many siblings never compacts under
    // any wrap rung, and TD relayout does not help a single rank.
    let wide = format!(
        "flowchart TD\nR[root]\n{}",
        (0..12)
            .map(|index| format!("R --> N{index}[sibling number {index}]"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    match render_mermaid(&wide, 60) {
        MermaidRender::Clipped {
            lines,
            hidden_columns,
        } => {
            assert!(hidden_columns > 0);
            let plain_lines = plain(&lines);
            assert!(plain_lines.iter().all(|line| display_width(line) <= 60));
            // The visible window must keep readable structure: node boxes and
            // edge ink. (A centered parent can land outside the window; see
            // the root-anchoring note in `render_clipped`.)
            let art = plain_lines.join("\n");
            assert!(art.contains("sibling number 0"), "{art}");
            assert!(art.contains('▼'), "{art}");
        }
        other => panic!("expected clip, got {other:?}"),
    }
    // Below the clip floor the source fallback remains.
    assert_eq!(
        render_mermaid(&wide, MIN_CLIP_WIDTH - 1),
        MermaidRender::Fallback(MermaidFallback::TooWide)
    );
}

// Covers: multi-word edge labels must wrap instead of forcing a source fallback
// Owner: terminal graph flow layout
#[test]
fn wraps_edge_labels_when_pane_is_tight() {
    let source = "flowchart TD\n\
        A[start] -->|when the renderer reports a width failure| B[fallback]";
    let lines = rendered(source, 46);
    let art = lines.join("\n");
    // The label must be present across rows rather than truncated to one row.
    assert!(art.contains("when the renderer"), "{art}");
    assert!(art.contains("width failure"), "{art}");
    assert!(lines.iter().all(|line| display_width(line) <= 46));
}

// Covers: a too-wide LR chain must restack as TD instead of dumping source
// Owner: mermaid flow layout
#[test]
fn relayouts_wide_lr_flowchart_to_td() {
    let lines = rendered(PHASE_CHAIN_FLOWCHART, 44);
    let phase1 = lines.iter().position(|line| line.contains("Phase 1"));
    let phase2 = lines.iter().position(|line| line.contains("Phase 2"));
    assert!(
        matches!((phase1, phase2), (Some(one), Some(two)) if one < two),
        "expected Phase 1 above Phase 2:\n{}",
        lines.join("\n")
    );
}

// Covers: sequence autonumber, alt frames, and activate must show in the art
// Owner: mermaid sequence layout
#[test]
fn sequence_paints_numbers_frames_and_activation() {
    let art = rendered(
        "sequenceDiagram\nautonumber\nparticipant A\nparticipant B\nactivate A\nA->>B: request\nalt success\nB->>A: ok\nelse failure\nB->>A: error\nend\ndeactivate A",
        80,
    )
    .join("\n");
    assert!(art.contains("1 request"), "{art}");
    assert!(art.contains("alt success"), "{art}");
    assert!(art.contains('┃'), "activation bar missing:\n{art}");
}

#[test]
fn renders_unicode_labels_without_mismeasuring_or_reordering_cells() {
    for direction in ["LR", "RL", "TD", "BT"] {
        let lines = rendered(
            &format!("flowchart {direction}\nA[你好] --> B[e\u{301}🙂👩\u{200d}💻]"),
            80,
        );
        let art = lines.join("\n");
        assert!(art.contains("你好"), "{direction}:\n{art}");
        assert!(
            art.contains("e\u{301}🙂👩\u{200d}💻"),
            "{direction}:\n{art}"
        );
        assert!(lines.iter().all(|line| display_width(line) <= 80));
    }
}

// Covers: empty gitGraph, gantt, and mindmap stay unsupported like empty sequence
// Owner: mermaid model conversion
#[test]
fn empty_gitgraph_gantt_and_mindmap_stay_unsupported() {
    for source in ["gitGraph", "gantt\ntitle Plan", "mindmap"] {
        assert_eq!(
            render_mermaid(source, 240),
            MermaidRender::Fallback(MermaidFallback::Unsupported),
            "{source}"
        );
    }
}

// Covers: gitGraph must show messages, tags, merges, and hide auto ids
// Owner: mermaid gitgraph layout
#[test]
fn gitgraph_paints_messages_merges_and_hides_auto_ids() {
    let art = rendered(
        "gitGraph\n\
            commit id: \"init\" msg: \"init\"\n\
            branch develop\n\
            commit id: \"parser\" msg: \"parser\" tag: \"wip\"\n\
            checkout main\n\
            merge develop\n\
            commit id: \"release\" msg: \"release\"",
        80,
    )
    .join("\n");
    assert!(art.contains("init"), "{art}");
    assert!(art.contains("parser"), "{art}");
    assert!(art.contains("wip"), "{art}");
    assert!(art.contains("merged branch develop into main"), "{art}");
    assert!(art.contains("release"), "{art}");
    assert!(art.contains('●') || art.contains('◉'), "{art}");

    let unlabeled = rendered("gitGraph\ncommit\ncommit", 80).join("\n");
    assert!(
        !unlabeled.contains('-'),
        "auto ids must not appear:\n{unlabeled}"
    );
}

// Covers: merge type: HIGHLIGHT must keep the join and use the highlight glyph
// Owner: mermaid gitgraph layout
#[test]
fn gitgraph_highlight_merge_keeps_join() {
    let art = rendered(
        "gitGraph\n\
            commit id: \"init\" msg: \"init\"\n\
            branch develop\n\
            commit id: \"parser\" msg: \"parser\"\n\
            checkout main\n\
            merge develop type: HIGHLIGHT",
        80,
    )
    .join("\n");
    assert!(art.contains('◆'), "highlight glyph missing:\n{art}");
    assert!(
        art.contains('─') || art.contains('├') || art.contains('└'),
        "merge join missing:\n{art}"
    );
}

// Covers: gantt after-chains and sections must keep later tasks to the right
// Owner: mermaid gantt layout
#[test]
fn gantt_paints_sections_and_orders_after_tasks() {
    let lines = rendered(
        "gantt\n\
            title Plan\n\
            section Build\n\
            Parser :p1, 3d\n\
            Painter :after p1, 2d",
        80,
    );
    let art = lines.join("\n");
    assert!(art.contains("Plan"), "{art}");
    assert!(art.contains("Build"), "{art}");
    let parser = lines.iter().find(|line| line.contains("Parser"));
    let painter = lines.iter().find(|line| line.contains("Painter"));
    let (Some(parser), Some(painter)) = (parser, painter) else {
        panic!("missing task rows:\n{art}");
    };
    let parser_bar = parser.find('█').or_else(|| parser.find('░'));
    let painter_bar = painter.find('█').or_else(|| painter.find('░'));
    assert!(
        matches!((parser_bar, painter_bar), (Some(left), Some(right)) if left < right),
        "expected Parser bar left of Painter:\n{art}"
    );
}

// Covers: mindmap children must indent under the root with tree guides
// Owner: mermaid mindmap layout
#[test]
fn mindmap_paints_indented_tree() {
    let lines = rendered("mindmap\n  Root\n    Child\n    Other", 80);
    let art = lines.join("\n");
    let root = lines.iter().position(|line| line.contains("Root"));
    let child = lines.iter().position(|line| line.contains("Child"));
    assert!(
        matches!((root, child), (Some(top), Some(below)) if top < below),
        "{art}"
    );
    let child_line = lines.iter().find(|line| line.contains("Child")).unwrap();
    assert!(
        child_line.contains('├') || child_line.contains('└'),
        "child missing tree guide:\n{art}"
    );
}

fn has_box_corner(line: &str) -> bool {
    line.chars()
        .any(|c| matches!(c, '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'))
}

fn marker_row(art: &[String], marker: &str) -> usize {
    art.iter()
        .position(|line| line.trim() == marker)
        .unwrap_or_else(|| panic!("missing {marker:?} line in\n{}", art.join("\n")))
}

fn boxed_label_row(art: &[String], label: &str) -> usize {
    art.iter()
        .position(|line| line.contains(label) && line.contains('│'))
        .unwrap_or_else(|| panic!("missing boxed {label:?} in\n{}", art.join("\n")))
}

// Covers: state [*] must not paint as an empty box; unique unlabeled start
// captions the first state, terminal [*] is a borderless word, and fan-out
// or labeled boot arrows keep a start stub.
// Owner: mermaid model conversion
#[test]
fn state_pseudostates_use_captions_and_text_stubs() {
    let unique = rendered(
        "stateDiagram-v2\n[*] --> Idle\nIdle --> Working: new task\nWorking --> Idle: done",
        80,
    );
    let start_row = marker_row(&unique, "start");
    assert!(
        !has_box_corner(&unique[start_row]),
        "start should sit outside the box:\n{}",
        unique.join("\n")
    );
    assert!(
        unique
            .get(start_row + 1)
            .is_some_and(|line| has_box_corner(line)),
        "start caption should sit on the Idle box:\n{}",
        unique.join("\n")
    );
    assert!(
        boxed_label_row(&unique, "Idle") > start_row,
        "Idle box should be under start:\n{}",
        unique.join("\n")
    );

    let with_end = rendered("stateDiagram-v2\n[*] --> Idle\nIdle --> [*]", 80);
    let end_row = marker_row(&with_end, "end");
    assert!(
        !has_box_corner(&with_end[end_row]),
        "end should be a borderless word:\n{}",
        with_end.join("\n")
    );

    let fan_out = rendered("stateDiagram-v2\n[*] --> A\n[*] --> B", 80);
    assert_eq!(
        fan_out.iter().filter(|line| line.trim() == "start").count(),
        1,
        "fan-out should keep one start stub:\n{}",
        fan_out.join("\n")
    );
    let start_row = marker_row(&fan_out, "start");
    assert!(
        !has_box_corner(&fan_out[start_row]),
        "fan-out start should be borderless:\n{}",
        fan_out.join("\n")
    );
    assert!(
        fan_out
            .get(start_row + 1)
            .is_none_or(|line| !has_box_corner(line)),
        "fan-out start must stay a stub, not a caption on A:\n{}",
        fan_out.join("\n")
    );
    let _ = boxed_label_row(&fan_out, "A");
    let _ = boxed_label_row(&fan_out, "B");

    let boot = rendered("stateDiagram-v2\n[*] --> Idle: boot", 80);
    let art = boot.join("\n");
    assert!(art.contains("boot"), "{art}");
    let start_row = marker_row(&boot, "start");
    assert!(
        !has_box_corner(&boot[start_row]),
        "labeled start should be borderless:\n{art}"
    );
    assert!(
        boot.get(start_row + 1)
            .is_none_or(|line| !has_box_corner(line)),
        "labeled start must stay a stub:\n{art}"
    );
    let _ = boxed_label_row(&boot, "Idle");
}
