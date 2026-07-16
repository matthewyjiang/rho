use super::*;

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
        MermaidRender::Fallback(reason) => panic!("unexpected fallback: {reason:?}"),
    }
}

#[test]
fn renders_each_supported_diagram_family_without_ansi_or_width_overflow() {
    let fixtures = [
        "flowchart LR\nA[Parse] --> B[Render]",
        "sequenceDiagram\nAlice->>Bob: Hello",
        "stateDiagram-v2\n[*] --> Ready",
        "pie\n\"Dogs\" : 5\n\"Cats\" : 3",
        "erDiagram\nCUSTOMER ||--o{ ORDER : places",
        "classDiagram\nAnimal <|-- Duck\nclass Animal",
        "journey\ntitle Day\nsection Work\nCode: 5: Me",
        "gantt\ntitle Plan\ndateFormat YYYY-MM-DD\nsection Work\nCode: 2025-01-01, 1d",
        "timeline\n2025 : Shipped",
        "gitGraph\ncommit id: \"one\"",
        "mindmap\n  root((Rho))\n    TUI",
        "quadrantChart\nFast: [0.8, 0.8]",
        "requirementDiagram\nrequirement test_req {\nid: 1\ntext: test\nrisk: low\nverifymethod: test\n}",
        "sankey-beta\nInput,Output,1",
        "xychart-beta\nx-axis [a, b]\nbar [1, 2]",
        "block-beta\ncolumns 2\nA B\nA --> B",
        "architecture-beta\nservice api(server)[API]\nservice db(database)[DB]\napi:R --> L:db",
        "packet-beta\n0-15: \"Source\"\n16-31: \"Destination\"",
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
fn applies_source_and_structural_limits_before_rendering() {
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
}

#[test]
fn rejects_blank_unsupported_unsafe_and_too_wide_sources() {
    assert_eq!(
        render_mermaid("  \n", 80),
        MermaidRender::Fallback(MermaidFallback::Blank)
    );
    assert_eq!(
        render_mermaid("unknownDiagram\nA", 80),
        MermaidRender::Fallback(MermaidFallback::Unsupported)
    );
    assert_eq!(
        render_mermaid("flowchart LR\nclick A \"https://example.com\"", 80),
        MermaidRender::Fallback(MermaidFallback::UnsafeContent)
    );
    assert_eq!(
        render_mermaid("flowchart LR\nA[a label that cannot fit]", 4),
        MermaidRender::Fallback(MermaidFallback::TooWide)
    );
}

#[test]
fn keeps_top_to_bottom_flowcharts_vertically_compact() {
    let lines = rendered(
        "flowchart TD\nA[Boil water] --> B[Place tea in cup] --> C[Pour in hot water] --> D[Drink]",
        100,
    );

    assert!(lines.iter().any(|line| line.contains("Boil water")));
    assert!(lines.iter().any(|line| line.contains("Drink")));
    assert!(
        lines.len() <= 24,
        "compact four-step flowchart used {} lines:\n{}",
        lines.len(),
        lines.join("\n")
    );
}

#[test]
fn keeps_branching_flowcharts_compact() {
    let lines = rendered(
        "flowchart TD\nA[Boil water] --> B[Place tea in cup]\nB --> C[Pour in hot water]\nC --> D{Add milk or sugar?}\nD -->|Yes| E[Add extras]\nD -->|No| F[Drink tea]\nE --> F",
        100,
    );

    assert!(lines.iter().any(|line| line.contains("Add milk or sugar?")));
    assert!(lines.iter().any(|line| line.contains("Drink tea")));
    assert!(
        lines.len() <= 32,
        "branching flowchart used {} lines:\n{}",
        lines.len(),
        lines.join("\n")
    );
}

#[test]
fn renders_unicode_labels_without_mismeasuring_cells() {
    let lines = rendered("flowchart LR\nA[你好] --> B[e\u{301}🙂]", 80);
    assert!(lines.iter().any(|line| line.contains('你')));
    assert!(lines.iter().any(|line| line.contains('好')));
    assert!(lines.iter().all(|line| display_width(line) <= 80));
}
