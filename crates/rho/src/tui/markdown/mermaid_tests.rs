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
    // A grouped label may fit the lossless model gate at the normal wrap but
    // exceed the line budget at narrower wraps. Fall back instead of truncating.
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

#[test]
fn still_refuses_kinds_and_labels_the_painter_cannot_hold() {
    let long_label = "x".repeat(WRAP_WIDTH * MAX_LINES + 1);
    let fixtures = [
        "sequenceDiagram".to_owned(),
        format!("flowchart TD\nA[{long_label}]"),
    ];

    for source in fixtures {
        assert_eq!(
            render_mermaid(&source, 240),
            MermaidRender::Fallback(MermaidFallback::Unsupported),
            "{source}"
        );
    }
}

// Covers: everyday agent diagrams must paint instead of collapsing to source
// Owner: mermaid paint policy
#[test]
fn ablation_fixtures_paint_or_name_the_remaining_fallback() {
    const AGENT_CHART: &str = r#"flowchart TD
    start[Closed mermaid fence] --> empty{Empty?}
    empty -->|yes| blank[Blank]
    empty -->|no| bytes{Over 64 KiB or 2048 lines?}
    bytes -->|yes| src[SourceBytes / SourceLines]
    bytes -->|no| safe{Width 0 or unsafe content?}
    safe -->|yes| uns[UnsafeContent]
    safe -->|no| header{Supported header?}
    header -->|no| unsup[Unsupported]
    header -->|yes| parse[parse_mermaid_strict]
    parse -->|err| mal[Malformed]
    parse -->|ok| policy{Painter policy}
    policy -->|RawFallback| unsup
    policy -->|paint| links{Node links?}
    links -->|yes| uns
    links -->|no| lossless{can_paint?}
    lossless -->|no| unsup
    lossless -->|yes| struct{Complexity over cap?}
    struct -->|yes| lim[StructuralLimit]
    struct -->|no| layout[Layout / paint]
    layout -->|Oversize Width| wide[TooWide]
    layout -->|Oversize Cells| cells[OutputCells]
    layout -->|ok| validate[validate_output]
    validate -->|too many lines / cells / ANSI / wider than pane| out[OutputLines / OutputCells / AnsiOutput / TooWide]
    validate -->|ok| art[Rendered art]
"#;

    let cases = [
        (
            "trivial_flow",
            "flowchart TD\nA --> B",
            80,
            /*rendered*/ true,
        ),
        (
            "styled_flow",
            "flowchart TD\nclassDef ok fill:#0f0\nA --> B\nclass A ok",
            80,
            true,
        ),
        (
            "cylinder_and_stadium",
            "flowchart TD\nA[(database)] --> B([api])",
            80,
            true,
        ),
        (
            "state_start_and_note",
            "stateDiagram-v2\n[*] --> Ready\nReady --> Waiting\nnote right of Ready: queued\nWaiting --> [*]",
            80,
            true,
        ),
        (
            "long_edge_label",
            "flowchart TD\nA -->|too many lines / cells / ANSI / wider than pane| B",
            80,
            true,
        ),
        (
            "long_group_title",
            "flowchart TD\nsubgraph abcdefghijklmnopqrstuvwxyz\nA[ok]\nend",
            80,
            true,
        ),
        (
            "parallel_edges",
            "flowchart TD\nA -->|one| B\nA -->|two| B",
            80,
            true,
        ),
        (
            "class_multiplicity",
            "classDiagram\nA \"1\" --> \"*\" B : owns",
            80,
            true,
        ),
        (
            "class_self_loop",
            "classDiagram\nA *-- A : contains",
            80,
            true,
        ),
        (
            "sequence_alt_autonumber_activate",
            "sequenceDiagram\nautonumber\nparticipant A\nparticipant B\nactivate A\nA->>B: request\nalt success\nB->>A: ok\nelse failure\nB->>A: error\nend\ndeactivate A",
            80,
            true,
        ),
        (
            "sequence_box",
            "sequenceDiagram\nbox Team\nparticipant A\nparticipant B\nend\nA->>B: hi",
            80,
            true,
        ),
        (
            "wide_lr_relayouts_to_td",
            PHASE_CHAIN_FLOWCHART,
            44,
            true,
        ),
        (
            "agent_chart_still_needs_width",
            AGENT_CHART,
            80,
            false,
        ),
        ("agent_chart_wide_pane", AGENT_CHART, 240, true),
        ("pie_still_unsupported", "pie\n\"Dogs\" : 5", 80, false),
        (
            "still_too_wide_when_a_node_cannot_fit",
            "flowchart LR\nA[a label that cannot fit]",
            4,
            false,
        ),
    ];

    for (name, source, width, should_paint) in cases {
        match render_mermaid(source, width) {
            MermaidRender::Rendered(lines) if should_paint => {
                assert!(!lines.is_empty(), "{name}");
                assert!(
                    lines.iter().all(|line| {
                        let text: String = line
                            .spans
                            .iter()
                            .map(|span| span.content.as_ref())
                            .collect();
                        !text.contains('\x1b') && display_width(&text) <= width
                    }),
                    "{name}"
                );
            }
            MermaidRender::Fallback(reason) if !should_paint => {
                assert!(
                    matches!(
                        reason,
                        MermaidFallback::Unsupported | MermaidFallback::TooWide
                    ),
                    "{name}: {reason:?}"
                );
            }
            other => panic!("{name}: unexpected {other:?}"),
        }
    }
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
