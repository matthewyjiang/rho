use ratatui::{
    style::Style,
    text::{Line, Span},
};

mod code_fence;
mod heading;
mod inline;
mod math;
mod mermaid;
mod panel;
mod stream;
mod table;
mod txm;

#[cfg(test)]
pub(crate) use mermaid::PHASE_CHAIN_FLOWCHART;

pub(in crate::tui) use code_fence::{
    is_closing_fence, opening_fence_info_token, parse_opening_fence, update_code_block_state,
    CodeFenceState,
};
use code_fence::{mermaid_opening_fence, CodeFence};

use super::markdown_image::standalone_markdown_image;
use super::syntax::BlockHighlighter;
use inline::{inline_markdown_stable_prefix_len, markdown_inline_segments, markdown_inline_text};
use panel::ClosedPanel;

pub(in crate::tui) use heading::HeadingLevel;
use heading::{heading_stream_state, parse_atx_heading, HeadingStreamState};
pub(super) use stream::{incremental_markdown_tail_start, markdown_stream_bounds};

#[cfg(test)]
#[path = "markdown/table_tests.rs"]
mod table_tests;

use super::{
    render::{
        char_display_width, display_width, hard_wrap_styled_spans, slice_spans_by_bytes,
        soft_wrap_visible_ranges, truncate_to_display_width,
        wrap_line_at_whitespace_ranges_with_protected_prefix,
    },
    theme::Theme,
};

pub(super) fn push_wrapped_markdown_without_copy_button_from_fence_state(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    state: &mut CodeFenceState,
) {
    lines.extend(
        render_markdown_from_fence_state(text, width, state, CodeBlockCopyButton::Hidden).lines,
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CodeBlockCopyButton {
    Visible,
    Hidden,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct MarkdownCodeBlock {
    pub(super) top_line: usize,
    pub(super) copy_columns: std::ops::Range<usize>,
    pub(super) text: String,
}

pub(super) struct RenderedMarkdown {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) code_blocks: Vec<MarkdownCodeBlock>,
    /// Standalone `![alt](path)` references, in source order.
    pub(super) image_sources: Vec<super::markdown_image::MarkdownImageSource>,
    /// Rendered fallback rows corresponding to `image_sources`.
    pub(super) image_rows: Vec<usize>,
}

/// Render-local open fenced block: fence marker, optional highlighter, and
/// optional copy-button capture. Open/close is a single state transition.
struct ActiveBlock<'a> {
    fence: CodeFence,
    highlighter: Option<BlockHighlighter>,
    copy: Option<ActiveCopyCapture<'a>>,
}

struct ActiveCopyCapture<'a> {
    top_line: usize,
    copy_columns: std::ops::Range<usize>,
    content: Vec<&'a str>,
}

pub(super) fn markdown_lines(
    text: &str,
    width: usize,
    state: &mut CodeFenceState,
) -> Vec<Line<'static>> {
    render_markdown(text, width, state).lines
}

pub(super) fn render_markdown(
    text: &str,
    width: usize,
    state: &mut CodeFenceState,
) -> RenderedMarkdown {
    render_markdown_from_fence_state(text, width, state, CodeBlockCopyButton::Visible)
}

fn render_markdown_from_fence_state(
    text: &str,
    width: usize,
    state: &mut CodeFenceState,
    copy_button: CodeBlockCopyButton,
) -> RenderedMarkdown {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut code_blocks = Vec::new();
    let mut image_sources = Vec::new();
    let mut image_rows = Vec::new();
    // Continue an open fence from a prior chunk (live preview). No header row:
    // that belongs to the opening line already committed above. Reuse the
    // stored highlighter so multi-line tokens keep their lexical state.
    let mut active = state.active.map(|fence| ActiveBlock {
        fence,
        highlighter: state.highlighter.take(),
        copy: None,
    });

    let raw_lines = text.lines().collect::<Vec<_>>();
    let mut line_index = 0;
    while line_index < raw_lines.len() {
        let raw_line = raw_lines[line_index];
        if active.is_none() {
            if let Some(opening) = mermaid_opening_fence(raw_line) {
                if let Some(closing_offset) = raw_lines[line_index + 1..]
                    .iter()
                    .position(|line| is_closing_fence(line, opening.fence))
                {
                    let closing_index = line_index + 1 + closing_offset;
                    let source = raw_lines[line_index + 1..closing_index].join("\n");
                    let panel = mermaid::render_closed_fence(source, width);
                    push_closed_panel(&mut lines, &mut code_blocks, copy_button, width, panel);
                    line_index = closing_index + 1;
                    continue;
                }
            }
            if let Some((source, consumed_lines)) =
                math::take_closed_display_math(&raw_lines[line_index..])
            {
                let panel = math::render_closed_display_math(source, width);
                push_closed_panel(&mut lines, &mut code_blocks, copy_button, width, panel);
                line_index += consumed_lines;
                continue;
            }
        }
        let opening_fence = active
            .is_none()
            .then(|| parse_opening_fence(raw_line))
            .flatten();
        let closing_fence = active
            .as_ref()
            .is_some_and(|block| is_closing_fence(raw_line, block.fence));
        if opening_fence.is_some() || closing_fence {
            if closing_fence {
                if let Some(ActiveBlock {
                    copy: Some(capture),
                    ..
                }) = active.take()
                {
                    code_blocks.push(MarkdownCodeBlock {
                        top_line: capture.top_line,
                        copy_columns: capture.copy_columns,
                        text: capture.content.join("\n"),
                    });
                } else {
                    active = None;
                }
                state.clear_open();
            } else {
                let fence = opening_fence.expect("opening branch");
                let language = opening_fence_info_token(raw_line);
                let label = language.as_deref().map(str::to_ascii_uppercase);
                let top_line = lines.len();
                lines.push(code_block_header(width, label.as_deref(), copy_button));
                let copy = (copy_button == CodeBlockCopyButton::Visible)
                    .then(|| code_block_copy_columns(width))
                    .flatten()
                    .map(|copy_columns| ActiveCopyCapture {
                        top_line,
                        copy_columns,
                        content: Vec::new(),
                    });
                // Seed language/active; take the highlighter onto the render-local
                // block so body lines advance one shared ParseState.
                state.open_fence(fence, language);
                let highlighter = state.highlighter.take();
                active = Some(ActiveBlock {
                    fence,
                    highlighter,
                    copy,
                });
            }
            line_index += 1;
            continue;
        }

        if let Some(block) = &mut active {
            if let Some(capture) = &mut block.copy {
                capture.content.push(raw_line);
            }
            let plain = Theme::code_text();
            let segments = match &mut block.highlighter {
                Some(highlighter) => highlighter
                    .highlight_line(raw_line)
                    .into_iter()
                    .map(|segment| {
                        let style = segment.style(plain);
                        StyledSegment::new(segment.text, style)
                    })
                    .collect(),
                None => vec![StyledSegment::new(raw_line.to_string(), plain)],
            };
            lines.extend(wrap_styled_segments_hard(&segments, width));
            line_index += 1;
            continue;
        }

        if let Some((table_lines, consumed_lines)) =
            table::markdown_table_lines(&raw_lines[line_index..], width)
        {
            lines.extend(table_lines);
            line_index += consumed_lines;
            continue;
        }

        if let Some(heading) = parse_atx_heading(raw_line) {
            lines.extend(markdown_heading_lines(heading, width));
            line_index += 1;
            continue;
        }

        if is_markdown_divider(raw_line) {
            lines.push(markdown_divider(width));
            line_index += 1;
            continue;
        }

        if let Some(image) = standalone_markdown_image(raw_line) {
            image_rows.push(lines.len());
            let fallback = if image.alt.is_empty() {
                format!("[image: {}]", image.path)
            } else {
                format!("[image: {}]", image.alt)
            };
            lines.push(Line::styled(fallback, Theme::markdown_link()));
            image_sources.push(image);
            line_index += 1;
            continue;
        }

        lines.extend(wrap_styled_segments(
            &markdown_inline_segments(raw_line),
            width,
        ));
        line_index += 1;
    }

    // Persist highlighter lexical state when the fence stays open across chunks.
    match active {
        Some(ActiveBlock {
            highlighter,
            copy: Some(capture),
            ..
        }) => {
            state.highlighter = highlighter;
            code_blocks.push(MarkdownCodeBlock {
                top_line: capture.top_line,
                copy_columns: capture.copy_columns,
                text: capture.content.join("\n"),
            });
        }
        Some(ActiveBlock { highlighter, .. }) => {
            state.highlighter = highlighter;
        }
        None => {
            // Closed path already cleared state; leave highlighter unset.
        }
    }

    if lines.is_empty() && text.is_empty() {
        lines.push(Line::from(Span::styled(String::new(), Theme::text())));
    }

    RenderedMarkdown {
        lines,
        code_blocks,
        image_sources,
        image_rows,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StyledSegment {
    text: String,
    style: Style,
}

impl StyledSegment {
    fn new(text: String, style: Style) -> Self {
        Self { text, style }
    }
}

fn is_markdown_divider(line: &str) -> bool {
    let trimmed = line.trim();
    let mut chars = trimmed.chars().filter(|ch| !ch.is_whitespace());
    let Some(marker) = chars.next() else {
        return false;
    };
    matches!(marker, '-' | '*' | '_')
        && trimmed.chars().filter(|ch| !ch.is_whitespace()).count() >= 3
        && chars.all(|ch| ch == marker)
}

fn markdown_divider(width: usize) -> Line<'static> {
    Line::from(Span::styled("─".repeat(width.max(1)), Theme::dim()))
}

/// Emit a closed art panel (mermaid, display math) with header and copy state.
fn push_closed_panel(
    lines: &mut Vec<Line<'static>>,
    code_blocks: &mut Vec<MarkdownCodeBlock>,
    copy_button: CodeBlockCopyButton,
    width: usize,
    panel: ClosedPanel,
) {
    let top_line = lines.len();
    let (title, body, source) = match panel {
        ClosedPanel::Art {
            title,
            lines: art,
            source,
        } => (title, panel::panel_lines(art, width), source),
        ClosedPanel::SourceFallback { title, source } => {
            let mut body = Vec::new();
            let plain = Theme::code_text();
            for content_line in source.lines() {
                let segments = vec![StyledSegment::new(content_line.to_string(), plain)];
                body.extend(wrap_styled_segments_hard(&segments, width));
            }
            if body.is_empty() {
                body.extend(wrap_styled_segments_hard(
                    &[StyledSegment::new(String::new(), plain)],
                    width,
                ));
            }
            (title, body, source)
        }
    };
    lines.push(code_block_header(width, Some(title), copy_button));
    lines.extend(body);
    push_copyable_code_block(code_blocks, copy_button, top_line, width, source);
}

fn push_copyable_code_block(
    code_blocks: &mut Vec<MarkdownCodeBlock>,
    copy_button: CodeBlockCopyButton,
    top_line: usize,
    width: usize,
    text: String,
) {
    if copy_button != CodeBlockCopyButton::Visible {
        return;
    }
    if let Some(copy_columns) = code_block_copy_columns(width) {
        code_blocks.push(MarkdownCodeBlock {
            top_line,
            copy_columns,
            text,
        });
    }
}

/// Slim header row above a code block or art panel: dim label on the left,
/// COPY right-aligned at the geometry [`code_block_copy_columns`] promises to
/// hit-testing. Always one row, even with no label and a hidden button, so
/// block line counts stay uniform.
fn code_block_header(
    width: usize,
    label: Option<&str>,
    copy_button: CodeBlockCopyButton,
) -> Line<'static> {
    let width = width.max(1);
    let copy_columns = (copy_button == CodeBlockCopyButton::Visible)
        .then(|| code_block_copy_columns(width))
        .flatten();
    let copy_label = copy_columns
        .as_ref()
        .and_then(|_| code_block_copy_label(width));
    // Keep at least one blank column between the label and COPY.
    let label_budget = copy_columns
        .as_ref()
        .map_or(width, |columns| columns.start.saturating_sub(1));
    let label = truncate_to_display_width(label.unwrap_or_default(), label_budget);
    let mut spans = Vec::new();
    if let Some(columns) = &copy_columns {
        let filler = columns.start.saturating_sub(display_width(&label));
        spans.push(Span::styled(
            format!("{label}{}", " ".repeat(filler)),
            Theme::dim(),
        ));
    } else {
        spans.push(Span::styled(label.into_owned(), Theme::dim()));
    }
    if let Some(copy_label) = copy_label {
        spans.push(Span::styled(
            copy_label,
            Theme::markdown_code_copy_button(/*hovered*/ false),
        ));
    }
    Line::from(spans)
}

fn code_block_copy_label(width: usize) -> Option<&'static str> {
    if width >= 9 {
        Some(" COPY ")
    } else if width >= 6 {
        Some("COPY")
    } else {
        None
    }
}

fn code_block_copy_columns(width: usize) -> Option<std::ops::Range<usize>> {
    let label_width = display_width(code_block_copy_label(width)?);
    let start = width.saturating_sub(label_width + 1);
    Some(start..start + label_width)
}

/// Hard-wrap highlighted segments at display-width columns, preserving span
/// styles across breaks. Code needs hard wrapping; [`wrap_styled_segments`]
/// soft-wraps at whitespace and would reflow source lines.
fn wrap_styled_segments_hard(segments: &[StyledSegment], width: usize) -> Vec<Line<'static>> {
    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let spans = segments
        .iter()
        .map(|segment| Span::styled(segment.text.clone(), segment.style))
        .collect::<Vec<_>>();
    let empty_style = segments
        .first()
        .map(|segment| segment.style)
        .unwrap_or_else(Theme::code_text);
    hard_wrap_styled_spans(&text, &spans, width, empty_style)
        .into_iter()
        .map(Line::from)
        .collect()
}

fn markdown_heading_lines(heading: heading::AtxHeading<'_>, width: usize) -> Vec<Line<'static>> {
    let heading_style = Theme::markdown_heading(heading.level);
    if heading.content.is_empty() {
        return vec![Line::from(Span::styled(String::new(), heading_style))];
    }

    let segments = markdown_inline_segments(heading.content)
        .into_iter()
        .map(|segment| StyledSegment::new(segment.text, heading_style.patch(segment.style)))
        .collect::<Vec<_>>();
    wrap_styled_segments(&segments, width)
}

fn wrap_markdown_line_ranges(line: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let protected_prefix_end = markdown_list_body_start(line).unwrap_or_default();
    wrap_line_at_whitespace_ranges_with_protected_prefix(line, width, protected_prefix_end)
}

fn markdown_list_body_start(line: &str) -> Option<usize> {
    let trimmed = line.trim_start_matches(char::is_whitespace);
    let leading_whitespace_len = line.len() - trimmed.len();
    let marker_len = trimmed.find(char::is_whitespace)?;
    let marker = &trimmed[..marker_len];
    let is_list_marker = matches!(marker, "-" | "+" | "*")
        || marker.strip_suffix(['.', ')']).is_some_and(|digits| {
            (1..=9).contains(&digits.len()) && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !is_list_marker {
        return None;
    }

    let separator_len = trimmed[marker_len..]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .map(char::len_utf8)
        .sum::<usize>();
    let body_start = leading_whitespace_len + marker_len + separator_len;
    (body_start < line.len()).then_some(body_start)
}

fn wrap_styled_segments(segments: &[StyledSegment], width: usize) -> Vec<Line<'static>> {
    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let spans = segments
        .iter()
        .map(|segment| Span::styled(segment.text.clone(), segment.style))
        .collect::<Vec<_>>();

    let lines = soft_wrap_visible_ranges(&text, wrap_markdown_line_ranges(&text, width))
        .map(|range| {
            let chunk = slice_spans_by_bytes(&spans, range.start, range.end);
            if chunk.is_empty() {
                // Preserve an empty content row so underline/style state does not
                // leak from adjacent lines when a wrap yields no visible glyphs.
                Line::from(Span::styled(
                    String::new(),
                    Style::default().remove_modifier(ratatui::style::Modifier::UNDERLINED),
                ))
            } else {
                Line::from(chunk)
            }
        })
        .collect::<Vec<_>>();

    if lines.is_empty() {
        vec![Line::from(Span::styled(
            String::new(),
            Style::default().remove_modifier(ratatui::style::Modifier::UNDERLINED),
        ))]
    } else {
        lines
    }
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
