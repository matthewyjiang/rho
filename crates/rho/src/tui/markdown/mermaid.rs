use std::panic::AssertUnwindSafe;

use ratatui::text::Line;

use super::super::{render::display_width, theme::Theme};
use super::panel::ClosedPanel;
use crate::tui::terminal_graph::{GraphStyles, Oversize};

mod flow;
mod model;
mod policy;
mod security;
mod sequence;

pub(super) struct MermaidArt {
    pub(super) styled_lines: Vec<Line<'static>>,
    pub(super) plain_lines: Vec<String>,
}

const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_SOURCE_LINES: usize = 2_048;
const MAX_PRIMARY_ENTITIES: usize = 128;
const MAX_RELATIONSHIPS: usize = 512;
const MAX_GROUPS: usize = 24;
const MAX_DETAILS: usize = 1_024;
const MAX_RENDERED_LINES: usize = 4_096;
const MAX_RENDERED_CELLS: usize = 2_000_000;

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

impl MermaidFallback {
    pub(super) fn panel_title(self) -> &'static str {
        match self {
            Self::TooWide => "MERMAID · PANE TOO NARROW",
            Self::Unsupported => "MERMAID · UNSUPPORTED",
            Self::Malformed => "MERMAID · INVALID",
            Self::SourceBytes
            | Self::SourceLines
            | Self::StructuralLimit
            | Self::OutputLines
            | Self::OutputCells => "MERMAID · TOO LARGE",
            Self::Blank | Self::UnsafeContent | Self::Panic | Self::AnsiOutput => {
                "MERMAID · NOT RENDERED"
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum MermaidRender {
    Rendered(Vec<Line<'static>>),
    Fallback(MermaidFallback),
}

pub(super) fn render_closed_fence(source: String, inner_width: usize) -> ClosedPanel {
    match render_mermaid(&source, inner_width) {
        MermaidRender::Rendered(lines) => ClosedPanel::Art {
            title: "MERMAID",
            lines,
            source,
        },
        MermaidRender::Fallback(reason) => ClosedPanel::SourceFallback {
            title: reason.panel_title(),
            source,
        },
    }
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
    if inner_width == 0 || security::contains_unsafe_content(source) {
        return MermaidRender::Fallback(MermaidFallback::UnsafeContent);
    }
    if !is_supported_header(source) {
        return MermaidRender::Fallback(MermaidFallback::Unsupported);
    }

    let parsed = match mermaid_rs_renderer::parse_mermaid_strict(source) {
        Ok(parsed) => parsed,
        Err(_) => return MermaidRender::Fallback(MermaidFallback::Malformed),
    };
    let diagram_policy = policy::diagram_policy(parsed.graph.kind);
    if diagram_policy == policy::DiagramPolicy::RawFallback {
        return MermaidRender::Fallback(MermaidFallback::Unsupported);
    }
    if !parsed.graph.node_links.is_empty() {
        return MermaidRender::Fallback(MermaidFallback::UnsafeContent);
    }
    let (primary, relationships, groups, details) = model::complexity(&parsed.graph);
    if primary > MAX_PRIMARY_ENTITIES
        || relationships > MAX_RELATIONSHIPS
        || groups > MAX_GROUPS
        || details > MAX_DETAILS
    {
        return MermaidRender::Fallback(MermaidFallback::StructuralLimit);
    }

    let Some(model) = model::from_ir(&parsed.graph) else {
        return MermaidRender::Fallback(MermaidFallback::Unsupported);
    };
    let style = Theme::code_text();
    let styles = GraphStyles {
        border: style,
        node_text: style,
        edge: style,
        edge_label: style,
        node_styles: Vec::new(),
    };
    let result = layout_model(&model, diagram_policy, &styles, inner_width);
    let art = match result {
        Ok(art) => art,
        Err(Oversize::Width) => {
            return MermaidRender::Fallback(MermaidFallback::TooWide);
        }
        Err(Oversize::Cells) => {
            return MermaidRender::Fallback(MermaidFallback::OutputCells);
        }
    };
    if let Err(fallback) = validate_output(&art.plain_lines, inner_width) {
        return fallback;
    }
    MermaidRender::Rendered(art.styled_lines)
}

fn layout_model(
    model: &model::TerminalModel,
    diagram_policy: policy::DiagramPolicy,
    styles: &GraphStyles,
    inner_width: usize,
) -> Result<MermaidArt, Oversize> {
    match diagram_policy {
        policy::DiagramPolicy::PaintSequence => sequence::layout_sequence(
            model
                .sequence
                .as_ref()
                .expect("sequence policy has sequence model"),
            styles,
            Some(inner_width),
        ),
        policy::DiagramPolicy::PaintClass | policy::DiagramPolicy::PaintEr => flow::render_class(
            &model.graph,
            model
                .class_info
                .as_ref()
                .expect("class policy has class model"),
            styles,
            Some(inner_width),
        ),
        policy::DiagramPolicy::PaintFlow | policy::DiagramPolicy::PaintState => {
            flow::layout_flow(&model.graph, styles, Some(inner_width))
        }
        policy::DiagramPolicy::RawFallback => unreachable!("handled before model conversion"),
    }
}

fn validate_output(lines: &[String], inner_width: usize) -> Result<(), MermaidRender> {
    if lines.len() > MAX_RENDERED_LINES {
        return Err(MermaidRender::Fallback(MermaidFallback::OutputLines));
    }
    let mut cells = 0usize;
    for line in lines {
        if line.contains('\x1b') {
            return Err(MermaidRender::Fallback(MermaidFallback::AnsiOutput));
        }
        let width = display_width(line);
        if width > inner_width {
            return Err(MermaidRender::Fallback(MermaidFallback::TooWide));
        }
        cells = cells.saturating_add(width);
        if cells > MAX_RENDERED_CELLS {
            return Err(MermaidRender::Fallback(MermaidFallback::OutputCells));
        }
    }
    Ok(())
}

fn is_supported_header(source: &str) -> bool {
    let Some(header) = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("%%"))
        .and_then(|line| line.split_whitespace().next())
    else {
        return false;
    };
    matches!(
        header.to_ascii_lowercase().as_str(),
        "flowchart"
            | "graph"
            | "statediagram"
            | "statediagram-v2"
            | "sequencediagram"
            | "classdiagram"
            | "erdiagram"
    )
}

#[cfg(test)]
pub(crate) const PHASE_CHAIN_FLOWCHART: &str = concat!(
    "flowchart LR\n",
    "  P1[\"Phase 1: retention sweep\"] --> P2[\"Phase 2: parent link on disk\"]\n",
    "  P2 --> P3[\"Phase 3: session delete API + CLI\"]\n",
    "  P3 --> P4[\"Phase 4: TUI delete in resume picker\"]\n",
    "  P3 --> P5[\"Phase 5: nest runs under session\"]"
);

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
