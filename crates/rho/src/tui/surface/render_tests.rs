use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::*;
use crate::tui::surface::tree::SurfaceTree;

fn text(text: &str) -> Node {
    Node::Text {
        text: text.to_owned(),
        caption: false,
    }
}

fn drawn(root: Node, width: usize) -> Vec<String> {
    let view = SurfaceView {
        tree: Arc::new(SurfaceTree {
            root,
            focusables: Vec::new(),
        }),
        keyboard: Keyboard::Away { reachable: false },
    };
    surface_lines(&view, width)
        .into_iter()
        .map(|line| {
            let row: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            row.trim_end().to_owned()
        })
        .collect()
}

// Covers: a row must not overflow a narrow terminal, where it stacks its
// children, nor waste a wide one, where they sit side by side.
// Owner: pure layout
#[test]
fn row_lays_children_out_by_the_width_it_has() {
    let row = || Node::Row(vec![text("alpha beta gamma"), text("delta epsilon zeta")]);
    let cases: [(&str, usize, &[&str]); 3] = [
        (
            "natural widths fit",
            40,
            &["alpha beta gamma  delta epsilon zeta"],
        ),
        (
            "equal columns, each wrapped",
            30,
            &["alpha beta      delta epsilon", "gamma           zeta"],
        ),
        (
            "too narrow for columns",
            20,
            &["alpha beta gamma", "delta epsilon zeta"],
        ),
    ];
    for (name, width, expected) in cases {
        assert_eq!(drawn(row(), width), expected, "{name}");
    }
}
