use std::sync::Arc;

use pretty_assertions::assert_eq;

use super::*;
use crate::tui::surface::tree::SurfaceTree;

fn text(text: &str) -> Node {
    Node::Text {
        text: text.to_owned(),
        role: TextRole::Body,
    }
}

fn field(label: &str, value: &str) -> Node {
    Node::Field {
        focus: None,
        label: label.to_owned(),
        value: value.to_owned(),
        placeholder: String::new(),
        kind: FieldKind::Short,
        failures: Vec::new(),
    }
}

fn checkbox(label: &str) -> Node {
    Node::CheckBox {
        focus: None,
        label: label.to_owned(),
        checked: false,
        failures: Vec::new(),
    }
}

fn rows(root: Node, width: usize) -> Vec<String> {
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
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

/// What a surface draws inside its frame, given `width` columns there.
fn drawn(root: Node, width: usize) -> Vec<String> {
    let rows = rows(root, width + 4);
    rows[1..rows.len() - 1]
        .iter()
        .map(|row| {
            let inside = row.trim_start_matches("│ ").trim_end_matches(" │");
            inside.trim_end().to_owned()
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

// Covers: a column must keep a control's own lines together and set groups
// apart, or a form reads as one run of lines.
// Owner: pure layout
#[test]
fn column_sets_groups_apart_and_keeps_each_together() {
    let form = Node::Column(vec![
        text("Signup"),
        text("Two steps."),
        field("Name", "Ada"),
        checkbox("Email me"),
        checkbox("Text me"),
        Node::Divider,
        text("Done"),
    ]);
    assert_eq!(
        drawn(form, 12),
        [
            "Signup",
            "Two steps.",
            "",
            "Name",
            " Ada",
            "",
            "□ Email me",
            "□ Text me",
            "────────────",
            "Done",
        ]
    );
}

// Covers: a row wider than the transcript corrupts the rows around it, so
// no control may draw past the width it is given, however narrow.
// Owner: pure layout
#[test]
fn surface_stays_within_its_width() {
    let surface = || {
        Node::Card(Box::new(Node::Column(vec![
            text("**A title longer than a narrow terminal**"),
            field("Name", "a value longer than a narrow terminal"),
            checkbox("Email me updates"),
            Node::Tabs {
                focus: 0,
                titles: vec!["Inputs".to_owned(), "Media".to_owned()],
                active: 0,
                body: None,
            },
            Node::Slider {
                focus: None,
                label: "Seats".to_owned(),
                value: 3.0,
                min: 1.0,
                max: 10.0,
                failures: Vec::new(),
            },
            Node::Row(vec![
                Node::Button {
                    focus: 1,
                    label: "Cancel".to_owned(),
                    variant: ButtonVariant::Default,
                    failures: Vec::new(),
                },
                Node::Button {
                    focus: 2,
                    label: "Submit".to_owned(),
                    variant: ButtonVariant::Primary,
                    failures: Vec::new(),
                },
            ]),
        ])))
    };
    for width in 1..=40 {
        let widest = rows(surface(), width)
            .iter()
            .map(|row| display_width(row))
            .max();
        assert!(widest <= Some(width), "{widest:?} columns at width {width}");
    }
}
