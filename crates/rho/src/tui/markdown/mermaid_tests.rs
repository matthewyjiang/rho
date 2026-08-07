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
    assert_eq!(
        render_mermaid(
            "flowchart TD\nsubgraph abcdefghijklmnopqrst\nA[ok]\nend",
            16,
        ),
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
fn raw_falls_back_when_terminal_painter_cannot_preserve_semantics() {
    let long_label = "x".repeat(WRAP_WIDTH * MAX_LINES + 1);
    let fixtures = [
        "stateDiagram-v2\n[*] --> Ready\nReady --> [*]".to_owned(),
        "stateDiagram-v2\nReady --> Waiting\nnote right of Ready: queued".to_owned(),
        "classDiagram\nA \"1\" --> \"*\" B : owns".to_owned(),
        "classDiagram\nA *-- A : contains".to_owned(),
        "sequenceDiagram\nparticipant A\nparticipant B\nactivate A\nA->>B: work\ndeactivate A"
            .to_owned(),
        "sequenceDiagram\nautonumber\nA->>B: work".to_owned(),
        "sequenceDiagram\nbox Team\nparticipant A\nend".to_owned(),
        "sequenceDiagram\nA->>B: request\nalt success\nB->>A: ok\nelse failure\nB->>A: error\nend"
            .to_owned(),
        "sequenceDiagram".to_owned(),
        "flowchart TD\nA -->|one| B\nA -->|two| B".to_owned(),
        "flowchart TD\nA[(database)]".to_owned(),
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
