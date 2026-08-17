use super::*;
use crate::tui::terminal_graph::{MAX_LINES, WRAP_WIDTH};
use pretty_assertions::assert_eq;

fn rendered(source: &str, width: usize) -> Vec<String> {
    match render_mermaid(source, width) {
        MermaidRender::Rendered(lines) => lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect(),
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
        "gantt\ntitle Plan",
        "timeline\n2025 : Shipped",
        "gitGraph\ncommit id: \"one\"",
        "mindmap\n  root((Rho))",
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
