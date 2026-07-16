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
        MermaidRender::Fallback(reason) => {
            panic!("unexpected fallback for {source:?}: {reason:?}")
        }
    }
}

fn label_position(lines: &[String], label: &str) -> (usize, usize) {
    lines
        .iter()
        .enumerate()
        .find_map(|(row, line)| line.find(label).map(|column| (row, column)))
        .unwrap_or_else(|| panic!("missing {label:?} in:\n{}", lines.join("\n")))
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
fn preserves_supported_family_semantics() {
    let fixtures = [
        (
            "stateDiagram-v2\nReady --> Running: start\nRunning --> Done: finish",
            &["Ready", "Running", "Done", "start", "finish"][..],
        ),
        (
            "sequenceDiagram\nparticipant Alice as Client\nparticipant Bob as Server\nAlice->>Bob: Hello\nNote over Alice,Bob: greeting",
            &["Client", "Server", "Hello", "greeting"][..],
        ),
        (
            "classDiagram\nAnimal <|-- Duck\nclass Animal {\n+name: String\n+speak()\n}",
            &["Animal", "Duck", "+name: String", "+speak()"][..],
        ),
        (
            "erDiagram\nCUSTOMER ||--o{ ORDER : places\nCUSTOMER {\nstring name\n}",
            &["CUSTOMER", "ORDER", "places", "string name", "0..*"][..],
        ),
    ];
    for (source, expected) in fixtures {
        let art = rendered(source, 240).join("\n");
        for value in expected {
            assert!(art.contains(value), "missing {value:?} in:\n{art}");
        }
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
    let long_label = "x".repeat(super::painter::WRAP_WIDTH * super::painter::MAX_LINES + 1);
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
fn class_compartments_and_inheritance_remain_distinct() {
    let lines = rendered(
        "classDiagram\nAnimal <|-- Duck\nclass Animal {\n+name: String\n+speak()\n}",
        120,
    );
    let attr_row = label_position(&lines, "+name: String").0;
    let method_row = label_position(&lines, "+speak()").0;
    assert!(attr_row < method_row, "{}", lines.join("\n"));
    assert!(
        lines[attr_row + 1..method_row]
            .iter()
            .any(|line| line.contains('├') && line.contains('─')),
        "{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().any(|line| ['△', '▽', '◁', '▷']
            .iter()
            .any(|glyph| line.contains(*glyph))),
        "{}",
        lines.join("\n")
    );
}

#[test]
fn branching_tea_diagram_has_compact_clean_routing() {
    let lines = rendered(
        "flowchart TD\nA[Boil water] --> B[Place tea in cup]\nB --> C[Pour in hot water]\nC --> D{Add milk or sugar?}\nD -->|Yes| E[Add extras]\nD -->|No| F[Drink tea]\nE --> F",
        100,
    );
    let art = lines.join("\n");

    for label in [
        "Boil water",
        "Place tea in cup",
        "Pour in hot water",
        "Add milk or sugar?",
        "Add extras",
        "Drink tea",
    ] {
        assert_eq!(art.matches(label).count(), 1, "{art}");
    }
    assert_eq!(
        art,
        "    ┌────────────┐\n    │ Boil water │\n    └──────┬─────┘\n           │\n           ▼\n ┌──────────────────┐\n │ Place tea in cup │\n └─────────┬────────┘\n           │\n           ▼\n ┌───────────────────┐\n │ Pour in hot water │\n └─────────┬─────────┘\n           │\n           ▼\n◇────────────────────◇\n│ Add milk or sugar? ├────┐\n◇──────────┬─────────◇    │\n           │              │\n           ▼Yes           │\n    ┌────────────┐        │\n    │ Add extras │        │\n    └──────┬─────┘        │\n           │              │\n           ▼              │\n     ┌───────────┐     No │\n     │ Drink tea │◄───────┘\n     └───────────┘"
    );
    assert!(lines.len() <= 32, "used {} lines:\n{art}", lines.len());
}

#[test]
fn straight_chains_stay_aligned_and_compact() {
    let vertical = rendered(
        "flowchart TD\nA[One] --> B[Two] --> C[Three] --> D[Four]",
        100,
    );
    let positions = ["One", "Two", "Three", "Four"].map(|label| label_position(&vertical, label));
    assert!(positions.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert!(
        positions.iter().map(|position| position.1).max().unwrap()
            - positions.iter().map(|position| position.1).min().unwrap()
            <= 1
    );
    assert!(vertical.len() <= 24, "{}", vertical.join("\n"));

    let horizontal = rendered("flowchart LR\nA[One] --> B[Two] --> C[Three]", 100);
    let positions = ["One", "Two", "Three"].map(|label| label_position(&horizontal, label));
    assert!(positions.windows(2).all(|pair| pair[0].1 < pair[1].1));
}

#[test]
fn honors_all_flowchart_directions() {
    for direction in ["TD", "BT", "LR", "RL"] {
        let lines = rendered(
            &format!("flowchart {direction}\nA[First] --> B[Second]"),
            80,
        );
        let first = label_position(&lines, "First");
        let second = label_position(&lines, "Second");
        let ordered = match direction {
            "TD" => first.0 < second.0,
            "BT" => first.0 > second.0,
            "LR" => first.1 < second.1,
            "RL" => first.1 > second.1,
            _ => unreachable!(),
        };
        assert!(ordered, "{direction}:\n{}", lines.join("\n"));
    }
}

#[test]
fn renders_unicode_labels_without_mismeasuring_or_reordering_cells() {
    for direction in ["LR", "RL", "TD", "BT"] {
        let lines = rendered(
            &format!("flowchart {direction}\nA[你好] --> B[e\u{301}🙂]"),
            80,
        );
        let art = lines.join("\n");
        assert!(art.contains("你好"), "{direction}:\n{art}");
        assert!(art.contains("e\u{301}🙂"), "{direction}:\n{art}");
        assert!(lines.iter().all(|line| display_width(line) <= 80));
    }
}
