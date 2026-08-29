//! Ignored instrumentation: minimum renderable pane width and render cost per
//! mermaid fixture.
//!
//! Prints one row per fixture: the smallest `inner_width` that renders as art,
//! how many wider widths still fail (non-monotonic holes), and mean render
//! time. Run before and after changing layout budgets so width limits keep
//! receipts:
//!
//!   cargo test -p rho-coding-agent --lib mermaid_width_receipts -- \
//!     --ignored --nocapture

use std::time::Instant;

use super::{render_mermaid, MermaidRender, PHASE_CHAIN_FLOWCHART};

const MAX_PROBE_WIDTH: usize = 300;
const TIMING_WIDTH: usize = 120;
const TIMING_ITERS: u32 = 40;

struct Fixture {
    name: &'static str,
    source: &'static str,
}

/// Realistic agent-authored diagrams, including the shapes that historically
/// fell back to source: labeled decision branches and multi-word edge labels.
const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "branchy_td_labeled",
        source: "flowchart TD\n\
            D[delta arrives] --> N{new complete line in open mermaid fence?}\n\
            N -- no --> S[skip - no diagram work]\n\
            N -- yes --> R[render_mermaid on complete-line prefix]\n\
            R -- rendered --> C[cache as last-good lines]\n\
            C --> P[truncate to panel header, repaint diagram]\n\
            R -- malformed prefix --> L[keep showing last-good render]\n\
            F[fence closes] --> E[existing render_closed_fence path unchanged]",
    },
    Fixture {
        name: "phase_chain_lr",
        source: PHASE_CHAIN_FLOWCHART,
    },
    Fixture {
        name: "state_fallback_modes",
        source: "stateDiagram-v2\n\
            [*] --> Probing\n\
            Probing --> Probing: malformed prefix\n\
            Probing --> Diagram: prefix renders\n\
            Probing --> Latched: terminal fallback\n\
            Diagram --> Latched: terminal fallback\n\
            Probing --> Closed: fence closes\n\
            Diagram --> Closed: fence closes\n\
            Latched --> Closed: fence closes",
    },
    Fixture {
        name: "long_edge_label",
        source: "flowchart TD\n\
            A[start] -->|when the renderer reports a terminal width failure| B[fallback]\n\
            A -->|otherwise| C[art]",
    },
    Fixture {
        name: "sequence_typical",
        source: "sequenceDiagram\n\
            participant TUI\n\
            participant Cache\n\
            participant Renderer\n\
            TUI->>Cache: delta\n\
            Cache->>Renderer: complete prefix\n\
            Renderer-->>Cache: art lines\n\
            Cache-->>TUI: repaint tail",
    },
    Fixture {
        name: "class_small",
        source: "classDiagram\n\
            Animal <|-- Duck\n\
            class Animal {\n+name: String\n+speak()\n}\n\
            class Duck {\n+quack()\n}",
    },
    Fixture {
        name: "er_small",
        source: "erDiagram\n\
            CUSTOMER ||--o{ ORDER : places\n\
            ORDER ||--|{ LINE_ITEM : contains",
    },
];

fn renders(source: &str, width: usize) -> bool {
    matches!(
        render_mermaid(source, width),
        MermaidRender::Rendered(_) | MermaidRender::Clipped { .. }
    )
}

#[test]
#[ignore = "instrumentation: prints mermaid width/cost receipts"]
fn run_mermaid_width_receipts() {
    println!(
        "{:<24} {:>9} {:>6} {:>14}",
        "fixture", "min_width", "holes", "mean_render_us"
    );
    for fixture in FIXTURES {
        let min_width = (1..=MAX_PROBE_WIDTH).find(|&width| renders(fixture.source, width));
        let holes = min_width.map_or(0, |min| {
            (min..=MAX_PROBE_WIDTH)
                .filter(|&width| !renders(fixture.source, width))
                .count()
        });
        let start = Instant::now();
        for _ in 0..TIMING_ITERS {
            std::hint::black_box(render_mermaid(
                std::hint::black_box(fixture.source),
                TIMING_WIDTH,
            ));
        }
        let mean_us = start.elapsed().as_micros() / u128::from(TIMING_ITERS);
        let min_display = min_width.map_or_else(|| "never".to_owned(), |width| width.to_string());
        println!(
            "{:<24} {:>9} {:>6} {:>14}",
            fixture.name, min_display, holes, mean_us
        );
    }
}
