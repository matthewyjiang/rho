use pretty_assertions::assert_eq;
use ratatui::style::{Color, Style};

use super::{Edge, EdgeHead, Graph, Node, NodeStyle};

// Covers: node-specific state colors must survive shared graph layout and painting.
// Owner: terminal graph painter.
#[test]
fn preserves_per_node_styles_and_node_geometry() {
    let waiting = NodeStyle::new(
        Style::default().fg(Color::Blue),
        Style::default().fg(Color::Cyan),
    );
    let running = NodeStyle::new(
        Style::default().fg(Color::Red),
        Style::default().fg(Color::Yellow),
    );
    let graph = Graph::top_down(
        vec![
            Node::rectangular("Waiting node", waiting),
            Node::rectangular("Running node", running),
        ],
        vec![Edge::directed(0, 1)],
    )
    .unwrap();

    let art = graph.render(Style::default().fg(Color::DarkGray)).unwrap();
    let waiting_text = text_style(&art.lines, "Waiting node");
    let running_text = text_style(&art.lines, "Running node");

    assert_eq!(waiting_text, waiting.text);
    assert_eq!(running_text, running.text);
    assert_eq!(
        art.plain_lines
            .iter()
            .flat_map(|line| line.chars())
            .filter(|character| *character == '▼')
            .count(),
        1
    );
    assert!(art
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| { span.style == waiting.border && span.content.chars().any(is_box_drawing) }));
    assert!(art
        .lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| { span.style == running.border && span.content.chars().any(is_box_drawing) }));
    assert!(art.node_rects[0].y < art.node_rects[1].y);
}

// Covers: one grapheme must consume its measured terminal width even when it
// contains several positive-width Unicode scalars.
// Owner: terminal graph painter.
#[test]
fn paints_zwj_graphemes_without_shifting_node_borders() {
    let graph = Graph::top_down(
        vec![Node::rectangular("👩\u{200d}💻", NodeStyle::default())],
        Vec::new(),
    )
    .unwrap();

    let art = graph.render(Style::default()).unwrap();

    assert_eq!(
        art.plain_lines,
        vec![
            "┌────┐".to_owned(),
            "│ 👩\u{200d}💻 │".to_owned(),
            "└────┘".to_owned()
        ]
    );
}

// Covers: self-loop endpoint decorations must match the graph edge model.
// Owner: terminal graph painter.
#[test]
fn paints_self_loop_endpoint_decorations() {
    let mut edge = Edge::directed(0, 0);
    edge.head_to = EdgeHead::None;
    edge.head_from = EdgeHead::Circle;
    let graph = Graph::top_down(
        vec![Node::rectangular("Node", NodeStyle::default())],
        vec![edge],
    )
    .unwrap();

    let art = graph.render(Style::default()).unwrap();

    assert_eq!(
        art.plain_lines,
        vec![
            "┌──────┐".to_owned(),
            "│ Node │".to_owned(),
            "└────o─┘".to_owned(),
            "     ││".to_owned(),
            "     ╰╯".to_owned(),
        ]
    );
}

fn text_style(lines: &[ratatui::text::Line<'_>], needle: &str) -> Style {
    lines
        .iter()
        .flat_map(|line| &line.spans)
        .find(|span| span.content == needle)
        .map(|span| span.style)
        .expect("node label is rendered")
}

fn is_box_drawing(character: char) -> bool {
    matches!(character, '┌' | '┐' | '└' | '┘' | '─' | '│')
}
