use pretty_assertions::assert_eq;
use ratatui::text::Line;

use super::{LastGoodMermaid, MermaidStreamMode, OpenMermaidTail};
use crate::tui::markdown::{parse_opening_fence, StreamingMermaidPrefix};

fn tail(mode: MermaidStreamMode, last_good: bool) -> OpenMermaidTail {
    OpenMermaidTail {
        source_start: 0,
        committed_end: 0,
        header_line: 0,
        fence: parse_opening_fence("```").expect("opening fence"),
        mode,
        last_good: last_good.then(|| LastGoodMermaid {
            title: "MERMAID",
            art: vec![Line::from("kept")],
        }),
    }
}

fn diagram() -> StreamingMermaidPrefix {
    StreamingMermaidPrefix::Diagram {
        title: "MERMAID",
        lines: vec![Line::from("art")],
    }
}

// Covers: mermaid stream mode upgrades and latches at most once; transient
// failures keep last-good art; unsafe/terminal drop it.
// Owner: mermaid stream tail
#[test]
fn mermaid_stream_mode_is_monotone() {
    let cases = [
        (
            MermaidStreamMode::Probing,
            false,
            diagram(),
            MermaidStreamMode::Diagram,
            true,
        ),
        (
            MermaidStreamMode::Probing,
            false,
            StreamingMermaidPrefix::Transient,
            MermaidStreamMode::Probing,
            false,
        ),
        (
            MermaidStreamMode::Probing,
            false,
            StreamingMermaidPrefix::Terminal,
            MermaidStreamMode::Latched,
            false,
        ),
        (
            MermaidStreamMode::Probing,
            false,
            StreamingMermaidPrefix::Unsafe,
            MermaidStreamMode::Latched,
            false,
        ),
        (
            MermaidStreamMode::Diagram,
            true,
            diagram(),
            MermaidStreamMode::Diagram,
            true,
        ),
        (
            MermaidStreamMode::Diagram,
            true,
            StreamingMermaidPrefix::Transient,
            MermaidStreamMode::Diagram,
            true,
        ),
        (
            MermaidStreamMode::Diagram,
            true,
            StreamingMermaidPrefix::Terminal,
            MermaidStreamMode::Latched,
            false,
        ),
        (
            MermaidStreamMode::Diagram,
            true,
            StreamingMermaidPrefix::Unsafe,
            MermaidStreamMode::Latched,
            false,
        ),
        (
            MermaidStreamMode::Latched,
            false,
            diagram(),
            MermaidStreamMode::Latched,
            false,
        ),
        (
            MermaidStreamMode::Latched,
            false,
            StreamingMermaidPrefix::Transient,
            MermaidStreamMode::Latched,
            false,
        ),
    ];

    for (mode, had_last_good, prefix, expected_mode, expected_last_good) in cases {
        let mut stream = tail(mode, had_last_good);
        stream.apply(prefix);
        assert_eq!(stream.mode, expected_mode);
        assert_eq!(stream.last_good.is_some(), expected_last_good);
    }
}
