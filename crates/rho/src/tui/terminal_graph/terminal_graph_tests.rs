use pretty_assertions::assert_eq;
use ratatui::style::{Color, Style};

use super::{
    art_from_layout, layout_canvas, Compartment, Direction, Edge, EdgeHead, Graph, GraphArt,
    GraphError, GraphStyles, Node, NodeExtra, NodeStyle, RankOrdering, TextAlignment, WRAP_WIDTH,
};

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
        RankOrdering::PreserveInput,
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
        RankOrdering::PreserveInput,
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
        RankOrdering::PreserveInput,
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

// Covers: a compartment's alignment is explicit even when earlier
// compartments are empty.
// Owner: terminal graph painter.
#[test]
fn aligns_each_compartment_by_its_model() {
    let graph = Graph::top_down(
        vec![Node::rectangular("unused", NodeStyle::default())],
        Vec::new(),
        RankOrdering::PreserveInput,
    )
    .unwrap();
    let extras = [NodeExtra::Compartments(vec![
        Compartment {
            lines: Vec::new(),
            alignment: TextAlignment::Left,
        },
        Compartment {
            lines: vec!["T".to_owned()],
            alignment: TextAlignment::Center,
        },
        Compartment {
            lines: vec!["field".to_owned()],
            alignment: TextAlignment::Left,
        },
    ])];
    let layout = layout_canvas(&graph, &extras, None, WRAP_WIDTH).unwrap();
    let styles = GraphStyles::for_nodes(&graph.nodes, Style::default());
    let art = art_from_layout(&graph, layout, &styles);

    assert_eq!(
        art.plain_lines,
        vec![
            " ┌───────┐".to_owned(),
            " │   T   │".to_owned(),
            " ├───────┤".to_owned(),
            " │ field │".to_owned(),
            " └───────┘".to_owned(),
        ]
    );
}

// Covers: an empty graph is a valid empty canvas, not a cell-budget failure.
// Owner: terminal graph layout.
#[test]
fn renders_an_empty_graph_as_empty_art() {
    let art = Graph::top_down(Vec::new(), Vec::new(), RankOrdering::PreserveInput)
        .unwrap()
        .render(Style::default())
        .unwrap();

    assert_eq!(
        art,
        GraphArt {
            lines: Vec::new(),
            plain_lines: Vec::new(),
            width: 0,
            height: 0,
            node_rects: Vec::new(),
        }
    );
}

// Covers: workflow-sized layouts retain caller order while compact diagram
// layouts can opt into crossing minimization.
// Owner: terminal graph layout policy.
#[test]
fn applies_the_requested_rank_ordering_policy() {
    let nodes = vec![
        Node::rectangular("A", NodeStyle::default()),
        Node::rectangular("B", NodeStyle::default()),
        Node::rectangular("X", NodeStyle::default()),
        Node::rectangular("Y", NodeStyle::default()),
    ];
    let edges = vec![Edge::directed(0, 3), Edge::directed(1, 2)];
    let preserved = Graph::top_down(nodes.clone(), edges.clone(), RankOrdering::PreserveInput)
        .unwrap()
        .render(Style::default())
        .unwrap();
    let minimized = Graph::top_down(nodes, edges, RankOrdering::MinimizeCrossings)
        .unwrap()
        .render(Style::default())
        .unwrap();

    assert!(preserved.node_rects[2].x < preserved.node_rects[3].x);
    assert!(minimized.node_rects[3].x < minimized.node_rects[2].x);
}

// Covers: malformed adapters fail at the graph boundary instead of panicking
// during rank or route indexing.
// Owner: terminal graph model validation.
#[test]
fn rejects_invalid_edge_endpoints() {
    let error = Graph::top_down(
        vec![Node::rectangular("only", NodeStyle::default())],
        vec![Edge::directed(0, 1)],
        RankOrdering::PreserveInput,
    )
    .unwrap_err();

    assert_eq!(error, GraphError::InvalidEdgeEndpoint);
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

// Covers: every fan-in edge of one target shares a single bus row, so a
// complete bipartite dependency layer renders one arrow drop per target
// instead of weaving a target's edges across several rows.
// Owner: terminal graph bus track assignment.
#[test]
fn fan_in_edges_share_one_bus_row_per_target() {
    let style = NodeStyle::default();
    let nodes = vec![
        Node::rectangular("first", style),
        Node::rectangular("second", style),
        Node::rectangular("left", style),
        Node::rectangular("right", style),
    ];
    let edges = vec![
        Edge::directed(0, 2),
        Edge::directed(1, 2),
        Edge::directed(0, 3),
        Edge::directed(1, 3),
    ];
    let graph = Graph::top_down(nodes, edges, RankOrdering::PreserveInput).unwrap();

    let art = graph.render(Style::default()).unwrap();

    assert_eq!(
        art.plain_lines,
        vec![
            " ┌───────┐  ┌────────┐".to_owned(),
            " │ first │  │ second │".to_owned(),
            " └───┬───┘  └────┬───┘".to_owned(),
            "     ├───────────┤".to_owned(),
            "     ▼           ▼".to_owned(),
            " ┌──────┐    ┌───────┐".to_owned(),
            " │ left │    │ right │".to_owned(),
            " └──────┘    └───────┘".to_owned(),
        ]
    );
}

// Covers: a forward edge that skips a rank must reach its target through the
// right lane and the target's own fan-in bus row, instead of running through
// sibling boxes at the target's center row (detached arrow fragments) or a
// separate approach row that crosses unrelated edges.
// Owner: terminal graph skip-edge routing.
#[test]
fn rank_skipping_edge_drops_into_the_target_from_above() {
    let style = NodeStyle::default();
    let nodes = vec![
        Node::rectangular("setup", style),
        Node::rectangular("review", style),
        Node::rectangular("apply", style),
        Node::rectangular("skip", style),
    ];
    let edges = vec![
        Edge::directed(0, 1),
        Edge::directed(1, 2),
        Edge::directed(1, 3),
        Edge::directed(0, 3),
    ];
    let graph = Graph::top_down(nodes, edges, RankOrdering::PreserveInput).unwrap();

    let art = graph.render(Style::default()).unwrap();

    assert!(art
        .plain_lines
        .iter()
        .all(|line| !line.contains('\u{25c4}')));
    assert_eq!(
        art.plain_lines,
        vec![
            "         ┌───────┐".to_owned(),
            "         │ setup │".to_owned(),
            "         └───┬───┘".to_owned(),
            "          ┌──┴───────┐".to_owned(),
            "          ▼          │".to_owned(),
            "     ┌────────┐      │".to_owned(),
            "     │ review │      │".to_owned(),
            "     └────┬───┘      │".to_owned(),
            "     ┌────┤          │".to_owned(),
            "     │    └─────┬────┘".to_owned(),
            "     ▼          ▼".to_owned(),
            " ┌───────┐  ┌──────┐".to_owned(),
            " │ apply │  │ skip │".to_owned(),
            " └───────┘  └──────┘".to_owned(),
        ]
    );
}

// Covers: skip edges whose targets share the full source set must join the
// one merged fan-in bus row instead of adding approach rows that cross other
// edges, so shared ink always means joined edges.
// Owner: terminal graph skip-edge routing and bus track assignment.
#[test]
fn skip_edges_join_the_shared_fan_in_bus_row() {
    let style = NodeStyle::default();
    let nodes = vec![
        Node::rectangular("collect", style),
        Node::rectangular("boundaries", style),
        Node::rectangular("spaghetti", style),
        Node::rectangular("structure", style),
        Node::rectangular("apply", style),
        Node::rectangular("none", style),
    ];
    let edges = vec![
        Edge::directed(0, 1),
        Edge::directed(0, 2),
        Edge::directed(0, 3),
        Edge::directed(1, 4),
        Edge::directed(2, 4),
        Edge::directed(3, 4),
        Edge::directed(0, 4),
        Edge::directed(1, 5),
        Edge::directed(2, 5),
        Edge::directed(3, 5),
        Edge::directed(0, 5),
    ];
    let graph = Graph::top_down(nodes, edges, RankOrdering::PreserveInput).unwrap();

    let art = graph.render(Style::default()).unwrap();

    assert_eq!(
        art.plain_lines,
        vec![
            "                  ┌─────────┐".to_owned(),
            "                  │ collect │".to_owned(),
            "                  └─────┬───┘".to_owned(),
            "       ┌────────────────┼───────────────┬───────┐".to_owned(),
            "       ▼                ▼               ▼       │".to_owned(),
            "┌────────────┐    ┌───────────┐   ┌───────────┐ │".to_owned(),
            "│ boundaries │    │ spaghetti │   │ structure │ │".to_owned(),
            "└──────┬─────┘    └─────┬─────┘   └─────┬─────┘ │".to_owned(),
            "       └──────────┬─────┴────┬──────────┴───────┘".to_owned(),
            "                  ▼          ▼".to_owned(),
            "              ┌───────┐  ┌──────┐".to_owned(),
            "              │ apply │  │ none │".to_owned(),
            "              └───────┘  └──────┘".to_owned(),
        ]
    );
}

const WRAPPED_EDGE_LABEL: &str = "when the renderer reports a width failure";

fn labeled_edge(from: usize, to: usize, label: &str) -> Edge {
    Edge {
        label: Some(label.to_owned()),
        ..Edge::directed(from, to)
    }
}

fn art_text(graph: &Graph) -> String {
    graph
        .render(Style::default())
        .unwrap()
        .plain_lines
        .join("\n")
}

fn assert_wrapped_label_visible(art: &str) {
    assert!(
        art.contains("when the renderer") && art.contains("width failure"),
        "expected every wrapped word to remain visible:\n{art}"
    );
}

// Covers: LR forward labels wrap in the inter-column gap, including below a
// sibling in the same rank, without occupied cells dropping later rows.
// Owner: terminal graph LR placement.
#[test]
fn wraps_lr_forward_edge_labels_without_dropping_words() {
    let style = NodeStyle::default();
    let graph = Graph::from_parts(
        vec![
            Node::rectangular("start", style),
            Node::rectangular("top", style),
            Node::rectangular("end", style),
        ],
        vec![Edge::directed(0, 1), labeled_edge(0, 2, WRAPPED_EDGE_LABEL)],
        Direction::LeftRight,
        RankOrdering::PreserveInput,
    )
    .unwrap();

    assert_wrapped_label_visible(&art_text(&graph));
}

// Covers: TD back-edge labels wrap in the right-lane margin without clipping
// off the canvas top or stopping on occupied cells.
// Owner: terminal graph TD placement.
#[test]
fn wraps_td_back_edge_labels_without_dropping_words() {
    let style = NodeStyle::default();
    let graph = Graph::from_parts(
        vec![
            Node::rectangular("start", style),
            Node::rectangular("end", style),
        ],
        vec![Edge::directed(0, 1), labeled_edge(1, 0, WRAPPED_EDGE_LABEL)],
        Direction::TopDown,
        RankOrdering::PreserveInput,
    )
    .unwrap();

    assert_wrapped_label_visible(&art_text(&graph));
}

// Covers: LR back-route labels wrap above the bottom lane instead of stacking
// into node boxes that silently truncate the remaining words.
// Owner: terminal graph LR placement.
#[test]
fn wraps_lr_back_edge_labels_without_dropping_words() {
    let style = NodeStyle::default();
    let graph = Graph::from_parts(
        vec![
            Node::rectangular("start", style),
            Node::rectangular("end", style),
        ],
        vec![Edge::directed(0, 1), labeled_edge(1, 0, WRAPPED_EDGE_LABEL)],
        Direction::LeftRight,
        RankOrdering::PreserveInput,
    )
    .unwrap();

    assert_wrapped_label_visible(&art_text(&graph));
}
