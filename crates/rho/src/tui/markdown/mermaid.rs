use std::{collections::HashMap, panic::AssertUnwindSafe};

use mermaid_text::detect::{detect, DiagramKind};
use ratatui::{text::Line, text::Span};

use super::super::{render::display_width, theme::Theme};

const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_SOURCE_LINES: usize = 2_048;
const MAX_PRIMARY_ENTITIES: usize = 128;
const MAX_RELATIONSHIPS: usize = 512;
const MAX_GROUPS: usize = 24;
const MAX_NESTING_DEPTH: usize = 6;
const MAX_DETAILS: usize = 1_024;
const MAX_RENDERED_LINES: usize = 4_096;
const MAX_RENDERED_CELLS: usize = 2_000_000;
const COMPACT_GRAPH_GAPS: (usize, usize) = (2, 1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MermaidFallback {
    Blank,
    SourceBytes,
    SourceLines,
    UnsafeContent,
    Unsupported,
    Malformed,
    StructuralLimit,
    Panic,
    OutputLines,
    OutputCells,
    TooWide,
    AnsiOutput,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MermaidRender {
    Rendered(Vec<Line<'static>>),
    Fallback(MermaidFallback),
}

#[derive(Default)]
struct Complexity {
    primary: usize,
    relationships: usize,
    groups: usize,
    details: usize,
    depth: usize,
}

pub(super) fn render_mermaid(source: &str, inner_width: usize) -> MermaidRender {
    match std::panic::catch_unwind(AssertUnwindSafe(|| render_inner(source, inner_width))) {
        Ok(result) => result,
        Err(_) => MermaidRender::Fallback(MermaidFallback::Panic),
    }
}

fn render_inner(source: &str, inner_width: usize) -> MermaidRender {
    if source.trim().is_empty() {
        return MermaidRender::Fallback(MermaidFallback::Blank);
    }
    if source.len() > MAX_SOURCE_BYTES {
        return MermaidRender::Fallback(MermaidFallback::SourceBytes);
    }
    if source.lines().count() > MAX_SOURCE_LINES {
        return MermaidRender::Fallback(MermaidFallback::SourceLines);
    }
    if inner_width == 0 || contains_unsafe_content(source) {
        return MermaidRender::Fallback(MermaidFallback::UnsafeContent);
    }

    let kind = match detect(source) {
        Ok(kind) => kind,
        Err(mermaid_text::Error::UnsupportedDiagram(_)) => {
            return MermaidRender::Fallback(MermaidFallback::Unsupported);
        }
        Err(_) => return MermaidRender::Fallback(MermaidFallback::Malformed),
    };
    let complexity = match parse_complexity(source, kind) {
        Ok(complexity) => complexity,
        Err(_) => return MermaidRender::Fallback(MermaidFallback::Malformed),
    };
    if complexity.primary > MAX_PRIMARY_ENTITIES
        || complexity.relationships > MAX_RELATIONSHIPS
        || complexity.groups > MAX_GROUPS
        || complexity.details > MAX_DETAILS
        || complexity.depth > MAX_NESTING_DEPTH
    {
        return MermaidRender::Fallback(MermaidFallback::StructuralLimit);
    }

    let compact_graph = matches!(kind, DiagramKind::Flowchart | DiagramKind::State);
    let mut options = mermaid_text::RenderOptions {
        max_width: Some(inner_width),
        ascii: false,
        color: false,
        // Grok Build's terminal renderer uses a compact layered layout rather
        // than a general-purpose graph layout. The dependency's native backend
        // follows the same policy and avoids tall routing bands for simple
        // terminal flowcharts.
        backend: if compact_graph {
            mermaid_text::layout::LayoutBackend::Native
        } else {
            mermaid_text::layout::LayoutBackend::default()
        },
        gaps_override: compact_graph.then_some(COMPACT_GRAPH_GAPS),
    };
    let mut output = match mermaid_text::render_with_options(source, &options) {
        Ok(output) => output,
        Err(_) => return MermaidRender::Fallback(MermaidFallback::Malformed),
    };
    if compact_graph && output.lines().any(|line| display_width(line) > inner_width) {
        // Explicit gaps bypass the dependency's width compaction. Fall back to
        // its width-aware pipeline when a compact graph still does not fit.
        options.gaps_override = None;
        output = match mermaid_text::render_with_options(source, &options) {
            Ok(output) => output,
            Err(_) => return MermaidRender::Fallback(MermaidFallback::Malformed),
        };
    }
    if output.contains('\x1b') {
        return MermaidRender::Fallback(MermaidFallback::AnsiOutput);
    }

    let output_lines = output.lines().collect::<Vec<_>>();
    if output_lines.len() > MAX_RENDERED_LINES {
        return MermaidRender::Fallback(MermaidFallback::OutputLines);
    }
    let mut cells = 0;
    for line in &output_lines {
        let width = display_width(line);
        if width > inner_width {
            return MermaidRender::Fallback(MermaidFallback::TooWide);
        }
        cells += width;
        if cells > MAX_RENDERED_CELLS {
            return MermaidRender::Fallback(MermaidFallback::OutputCells);
        }
    }

    MermaidRender::Rendered(
        output_lines
            .into_iter()
            .map(|line| Line::from(Span::styled(line.to_owned(), Theme::markdown_code_block())))
            .collect(),
    )
}

fn contains_unsafe_content(source: &str) -> bool {
    if source.contains('\x1b') {
        return true;
    }
    let lower = source.to_ascii_lowercase();
    lower.contains("javascript:")
        || lower.contains("<script")
        || lower.contains("<iframe")
        || lower.contains("<a ")
        || lower.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("click ") || trimmed.starts_with("href ")
        })
}

fn parse_complexity(source: &str, kind: DiagramKind) -> Result<Complexity, mermaid_text::Error> {
    let complexity = match kind {
        DiagramKind::Flowchart => graph_complexity(mermaid_text::parser::flowchart::parse(source)?),
        DiagramKind::State => graph_complexity(mermaid_text::parser::state::parse(source)?),
        DiagramKind::Sequence => {
            let diagram = mermaid_text::parser::sequence::parse(source)?;
            Complexity {
                primary: diagram.participants.len(),
                relationships: diagram.messages.len(),
                groups: diagram.blocks.len() + diagram.participant_groups.len(),
                details: diagram.notes.len() + diagram.activations.len(),
                depth: sequence_depth(&diagram.blocks),
            }
        }
        DiagramKind::Pie => {
            let chart = mermaid_text::parser::pie::parse(source)?;
            Complexity {
                primary: chart.slices.len(),
                ..Default::default()
            }
        }
        DiagramKind::Er => {
            let diagram = mermaid_text::parser::er::parse(source)?;
            Complexity {
                primary: diagram.entities.len(),
                relationships: diagram.relationships.len(),
                details: diagram
                    .entities
                    .iter()
                    .map(|entity| entity.attributes.len())
                    .sum(),
                ..Default::default()
            }
        }
        DiagramKind::Class => {
            let diagram = mermaid_text::parser::class::parse(source)?;
            Complexity {
                primary: diagram.classes.len(),
                relationships: diagram.relations.len(),
                details: diagram
                    .classes
                    .iter()
                    .map(|class| class.members.len())
                    .sum(),
                ..Default::default()
            }
        }
        DiagramKind::Journey => {
            let diagram = mermaid_text::parser::journey::parse(source)?;
            Complexity {
                primary: diagram
                    .sections
                    .iter()
                    .map(|section| section.tasks.len())
                    .sum(),
                groups: diagram.sections.len(),
                ..Default::default()
            }
        }
        DiagramKind::Gantt => {
            let diagram = mermaid_text::parser::gantt::parse(source)?;
            Complexity {
                primary: diagram
                    .sections
                    .iter()
                    .map(|section| section.tasks.len())
                    .sum(),
                groups: diagram.sections.len(),
                ..Default::default()
            }
        }
        DiagramKind::Timeline => {
            let diagram = mermaid_text::parser::timeline::parse(source)?;
            Complexity {
                primary: diagram
                    .sections
                    .iter()
                    .map(|section| section.entries.len())
                    .sum(),
                groups: diagram.sections.len(),
                details: diagram
                    .sections
                    .iter()
                    .flat_map(|section| &section.entries)
                    .map(|entry| entry.events.len())
                    .sum(),
                ..Default::default()
            }
        }
        DiagramKind::GitGraph => {
            let graph = mermaid_text::parser::git_graph::parse(source)?;
            Complexity {
                primary: graph.commits.len(),
                groups: graph.branches.len(),
                details: graph.events.len(),
                ..Default::default()
            }
        }
        DiagramKind::Mindmap => {
            let map = mermaid_text::parser::mindmap::parse(source)?;
            let (nodes, depth) = mindmap_complexity(&map.root);
            Complexity {
                primary: nodes,
                depth,
                ..Default::default()
            }
        }
        DiagramKind::QuadrantChart => {
            let chart = mermaid_text::parser::quadrant_chart::parse(source)?;
            Complexity {
                primary: chart.points.len(),
                ..Default::default()
            }
        }
        DiagramKind::RequirementDiagram => {
            let diagram = mermaid_text::parser::requirement_diagram::parse(source)?;
            Complexity {
                primary: diagram.requirements.len() + diagram.elements.len(),
                relationships: diagram.relationships.len(),
                ..Default::default()
            }
        }
        DiagramKind::Sankey => {
            let diagram = mermaid_text::parser::sankey::parse(source)?;
            let mut entities = std::collections::HashSet::new();
            for flow in &diagram.flows {
                entities.insert(&flow.source);
                entities.insert(&flow.target);
            }
            Complexity {
                primary: entities.len(),
                relationships: diagram.flows.len(),
                ..Default::default()
            }
        }
        DiagramKind::XyChart => {
            let chart = mermaid_text::parser::xy_chart::parse(source)?;
            Complexity {
                primary: chart.bar_series.len() + chart.line_series.len(),
                ..Default::default()
            }
        }
        DiagramKind::BlockDiagram => {
            let diagram = mermaid_text::parser::block_diagram::parse(source)?;
            Complexity {
                primary: diagram.blocks.len(),
                relationships: diagram.edges.len(),
                ..Default::default()
            }
        }
        DiagramKind::Architecture => {
            let diagram = mermaid_text::parser::architecture::parse(source)?;
            Complexity {
                primary: diagram.services.len(),
                relationships: diagram.edges.len(),
                groups: diagram.groups.len(),
                ..Default::default()
            }
        }
        DiagramKind::Packet => {
            let packet = mermaid_text::parser::packet::parse(source)?;
            Complexity {
                primary: packet.fields.len(),
                ..Default::default()
            }
        }
    };
    Ok(complexity)
}

fn graph_complexity(graph: mermaid_text::Graph) -> Complexity {
    Complexity {
        primary: graph.nodes.len(),
        relationships: graph.edges.len(),
        groups: graph.subgraphs.len(),
        depth: subgraph_depth(&graph.subgraphs),
        ..Default::default()
    }
}

fn subgraph_depth(subgraphs: &[mermaid_text::types::Subgraph]) -> usize {
    let by_id = subgraphs
        .iter()
        .map(|group| (group.id.as_str(), group))
        .collect::<HashMap<_, _>>();
    let child_ids = subgraphs
        .iter()
        .flat_map(|group| group.subgraph_ids.iter().map(String::as_str))
        .collect::<std::collections::HashSet<_>>();
    let mut stack = subgraphs
        .iter()
        .filter(|group| !child_ids.contains(group.id.as_str()))
        .map(|group| (group, 1))
        .collect::<Vec<_>>();
    if stack.is_empty() && !subgraphs.is_empty() {
        return MAX_NESTING_DEPTH + 1;
    }

    let mut visited = std::collections::HashSet::new();
    let mut max_depth = 0;
    while let Some((group, depth)) = stack.pop() {
        if !visited.insert(group.id.as_str()) {
            return MAX_NESTING_DEPTH + 1;
        }
        max_depth = max_depth.max(depth);
        stack.extend(
            group
                .subgraph_ids
                .iter()
                .filter_map(|id| by_id.get(id.as_str()))
                .map(|child| (*child, depth + 1)),
        );
    }
    max_depth
}

fn mindmap_complexity(root: &mermaid_text::MindmapNode) -> (usize, usize) {
    let mut stack = vec![(root, 1)];
    let mut nodes = 0;
    let mut max_depth = 0;
    while let Some((node, depth)) = stack.pop() {
        nodes += 1;
        max_depth = max_depth.max(depth);
        stack.extend(node.children.iter().map(|child| (child, depth + 1)));
    }
    (nodes, max_depth)
}

fn sequence_depth(blocks: &[mermaid_text::sequence::Block]) -> usize {
    // The dependency exposes sequence blocks as a flat list. Overlapping ranges
    // are nested when one block's message interval strictly contains another.
    blocks
        .iter()
        .map(|outer| {
            1 + blocks
                .iter()
                .filter(|inner| {
                    outer.start_message <= inner.start_message
                        && outer.end_message >= inner.end_message
                        && (outer.start_message, outer.end_message)
                            != (inner.start_message, inner.end_message)
                })
                .count()
        })
        .max()
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
