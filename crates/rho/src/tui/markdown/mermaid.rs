use std::panic::AssertUnwindSafe;

use ratatui::text::{Line, Span};

use super::super::{
    render::{display_width, truncate_to_display_width},
    theme::Theme,
};
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
/// Narrowest pane where a horizontally clipped diagram still reads as one.
/// Receipt: `mermaid_width_receipts` measures real fixtures rendering fully at
/// 18-70 columns; below roughly one tight node box plus a border (12+4 wrap
/// plus frame) a clipped canvas carries no structure, so source wins.
const MIN_CLIP_WIDTH: usize = 24;

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
    /// Art laid out wider than the pane and cut at the right edge. The marker
    /// row appended by [`render_closed_fence`] names the hidden columns; COPY
    /// still yields the full source.
    Clipped {
        lines: Vec<Line<'static>>,
        hidden_columns: usize,
    },
    Fallback(MermaidFallback),
}

pub(super) fn render_closed_fence(source: String, inner_width: usize) -> ClosedPanel {
    match mermaid_art(&source, inner_width) {
        Ok((title, lines)) => ClosedPanel::Art {
            title,
            lines,
            source,
        },
        Err(reason) => ClosedPanel::SourceFallback {
            title: reason.panel_title(),
            source,
        },
    }
}

/// Live mermaid for an unclosed fence.
///
/// Tries the complete-line prefix, then walks back through earlier complete
/// lines on blank/malformed so a later bad line keeps last-good art. Sticky
/// failures (unsafe, unsupported, too large) return `None` so the caller keeps
/// an ordinary source fence until close.
pub(super) fn render_open_prefix(
    complete_body: &str,
    copy_source: &str,
    inner_width: usize,
) -> Option<ClosedPanel> {
    let mut body = complete_body;
    loop {
        match mermaid_art(body, inner_width) {
            Ok((title, lines)) => {
                return Some(ClosedPanel::Art {
                    title,
                    lines,
                    source: copy_source.to_string(),
                });
            }
            Err(MermaidFallback::Blank | MermaidFallback::Malformed) => {
                let (shorter, _) = body.rsplit_once('\n')?;
                body = shorter;
            }
            Err(_) => return None,
        }
    }
}

fn mermaid_art(
    source: &str,
    inner_width: usize,
) -> Result<(&'static str, Vec<Line<'static>>), MermaidFallback> {
    match render_mermaid(source, inner_width) {
        MermaidRender::Rendered(lines) => Ok(("MERMAID", lines)),
        MermaidRender::Clipped {
            mut lines,
            hidden_columns,
        } => {
            lines.push(Line::from(Span::styled(
                format!("▶ {hidden_columns} cols clipped"),
                Theme::dim(),
            )));
            Ok(("MERMAID · CLIPPED", lines))
        }
        MermaidRender::Fallback(reason) => Err(reason),
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
    let result = layout_model(&model, diagram_policy, &styles, Some(inner_width));
    let art = match result {
        Ok(art) => art,
        Err(Oversize::Width) => {
            return render_clipped(&model, diagram_policy, &styles, inner_width);
        }
        Err(Oversize::Cells) => {
            return MermaidRender::Fallback(MermaidFallback::OutputCells);
        }
    };
    if let Err(reason) = validate_output(&art.plain_lines, /*max_width*/ Some(inner_width)) {
        return MermaidRender::Fallback(reason);
    }
    MermaidRender::Rendered(art.styled_lines)
}

/// Failure-path floor: lay the diagram out without a width cap and cut it at
/// the pane's right edge. Runs only after every bounded compaction rung fails,
/// so fitting diagrams pay nothing. Cell/line caps still bound the unbounded
/// layout.
///
/// Known limit: layout centers parents over their children, so on very wide
/// single ranks a centered node can fall right of the window. The clip keeps
/// the left edge because TD flow anchors entry nodes there; a follow-the-root
/// window is future work if receipts show it matters.
fn render_clipped(
    model: &model::TerminalModel,
    diagram_policy: policy::DiagramPolicy,
    styles: &GraphStyles,
    inner_width: usize,
) -> MermaidRender {
    if inner_width < MIN_CLIP_WIDTH {
        return MermaidRender::Fallback(MermaidFallback::TooWide);
    }
    let art = match layout_model(model, diagram_policy, styles, /*max_width*/ None) {
        Ok(art) => art,
        // Unbounded layout cannot fail on width; anything else keeps the
        // pre-clip fallback taxonomy.
        Err(Oversize::Width) => return MermaidRender::Fallback(MermaidFallback::TooWide),
        Err(Oversize::Cells) => return MermaidRender::Fallback(MermaidFallback::OutputCells),
    };
    let full_width = art
        .plain_lines
        .iter()
        .map(|line| display_width(line))
        .max()
        .unwrap_or(0);
    let hidden_columns = full_width.saturating_sub(inner_width);
    if hidden_columns == 0 {
        // Bounded layout failed but the unbounded one fits: still art.
        return MermaidRender::Rendered(art.styled_lines);
    }
    let lines: Vec<Line<'static>> = art
        .styled_lines
        .into_iter()
        .map(|line| clip_line_to_width(line, inner_width))
        .collect();
    let plain_clipped: Vec<String> = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    if let Err(reason) = validate_output(&plain_clipped, /*max_width*/ None) {
        return MermaidRender::Fallback(reason);
    }
    MermaidRender::Clipped {
        lines,
        hidden_columns,
    }
}

/// Cut a styled line at a display-column boundary, preserving span styles.
fn clip_line_to_width(line: Line<'static>, width: usize) -> Line<'static> {
    let mut used = 0usize;
    let mut spans = Vec::new();
    for span in line.spans {
        if used >= width {
            break;
        }
        let span_width = display_width(span.content.as_ref());
        if used + span_width <= width {
            used += span_width;
            spans.push(span);
        } else {
            let cut = truncate_to_display_width(span.content.as_ref(), width - used).into_owned();
            spans.push(Span::styled(cut, span.style));
            // A dropped trailing double-width grapheme can leave a leftover
            // column. Everything after this span is past the boundary.
            break;
        }
    }
    Line::from(spans)
}

fn layout_model(
    model: &model::TerminalModel,
    diagram_policy: policy::DiagramPolicy,
    styles: &GraphStyles,
    max_width: Option<usize>,
) -> Result<MermaidArt, Oversize> {
    match diagram_policy {
        policy::DiagramPolicy::PaintSequence => sequence::layout_sequence(
            model
                .sequence
                .as_ref()
                .expect("sequence policy has sequence model"),
            styles,
            max_width,
        ),
        policy::DiagramPolicy::PaintClass | policy::DiagramPolicy::PaintEr => flow::render_class(
            &model.graph,
            model
                .class_info
                .as_ref()
                .expect("class policy has class model"),
            styles,
            max_width,
        ),
        policy::DiagramPolicy::PaintFlow | policy::DiagramPolicy::PaintState => {
            flow::layout_flow(&model.graph, styles, max_width)
        }
        policy::DiagramPolicy::RawFallback => unreachable!("handled before model conversion"),
    }
}

fn validate_output(lines: &[String], max_width: Option<usize>) -> Result<(), MermaidFallback> {
    if lines.len() > MAX_RENDERED_LINES {
        return Err(MermaidFallback::OutputLines);
    }
    let mut cells = 0usize;
    for line in lines {
        if line.contains('\x1b') {
            return Err(MermaidFallback::AnsiOutput);
        }
        let width = display_width(line);
        if max_width.is_some_and(|max| width > max) {
            return Err(MermaidFallback::TooWide);
        }
        cells = cells.saturating_add(width);
        if cells > MAX_RENDERED_CELLS {
            return Err(MermaidFallback::OutputCells);
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

#[cfg(test)]
#[path = "mermaid_width_receipts.rs"]
mod width_receipts;
