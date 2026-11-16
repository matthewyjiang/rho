//! Draws a surface as transcript lines: a function of its tree, where the
//! keyboard is, and the width.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use super::{
    tree::{ButtonVariant, ChoiceOption, Control, Failure, FieldKind, MediaKind, Node, TextRole},
    Editing, Keyboard, SurfaceView,
};
use crate::tui::{
    markdown::{render_markdown, CodeFenceState},
    render::{clip_line, display_width, push_wrapped_text, spans_display_width, LineFill},
    theme::Theme,
};

/// Columns between the children of a row.
const ROW_GAP: usize = 2;

/// The narrowest column a row gives a child before it stacks its children
/// instead.
const MIN_COLUMN: usize = 12;

/// The widest a surface draws, so a form stays readable on a wide terminal.
const MAX_WIDTH: usize = 80;

/// The widest a field's value area draws.
const MAX_FIELD: usize = 48;

/// The narrowest width that still has room for a frame around content.
const MIN_FRAMED: usize = 6;

/// Cells in a slider's track.
const SLIDER_TRACK: usize = 20;

/// The lines of `view`, each at most `width` columns wide: the surface in a
/// frame, then the keys that apply to it.
pub(in crate::tui) fn surface_lines(view: &SurfaceView, width: usize) -> Vec<Line<'static>> {
    let painter = Painter {
        view,
        focused: match &view.keyboard {
            Keyboard::On { index, .. } => Some(*index),
            Keyboard::Away { .. } => None,
        },
    };
    let width = width.clamp(1, MAX_WIDTH);
    // A card at the root is the surface's own frame, not a second one.
    let content = match &view.tree.root {
        Node::Card(content) => content,
        root => root,
    };
    let border = if painter.focused.is_some() {
        Theme::accent()
    } else {
        Theme::dim()
    };
    let mut lines = framed(painter.card(content, frame_inner(width)), width, border);
    if let Some(hint) = painter.hint() {
        push_wrapped_text(&mut lines, hint, width, Theme::dim(), LineFill::Natural);
    }
    lines
        .into_iter()
        .map(|line| clip_line(line, width))
        .collect()
}

/// The glyph drawn for the basic catalog's icon `name`.
pub(super) fn icon_glyph(name: &str) -> &'static str {
    match name {
        "accountCircle" | "person" => "☺",
        "add" => "+",
        "arrowBack" => "←",
        "arrowForward" => "→",
        "attachFile" => "⌁",
        "calendarToday" | "event" => "▦",
        "call" | "phone" => "☎",
        "camera" | "photo" => "◙",
        "check" => "✓",
        "close" => "✕",
        "delete" => "⌫",
        "download" => "↓",
        "upload" => "↑",
        "edit" => "✎",
        "error" => "✖",
        "warning" => "⚠",
        "info" => "ℹ",
        "help" => "?",
        "favorite" | "star" => "★",
        "favoriteOff" | "starOff" => "☆",
        "starHalf" => "⯪",
        "folder" => "▤",
        "home" => "⌂",
        "locationOn" => "⌖",
        "lock" => "▣",
        "lockOpen" => "▢",
        "mail" | "send" => "✉",
        "menu" => "≡",
        "moreVert" => "⋮",
        "moreHoriz" => "⋯",
        "notifications" => "♪",
        "notificationsOff" => "♩",
        "play" => "▶",
        "pause" => "‖",
        "stop" => "■",
        "fastForward" | "skipNext" => "»",
        "rewind" | "skipPrevious" => "«",
        "payment" | "shoppingCart" => "¤",
        "print" => "⎙",
        "refresh" => "↻",
        "search" => "⌕",
        "settings" => "⚙",
        "share" => "⇪",
        "visibility" => "◉",
        "visibilityOff" => "◎",
        "volumeUp" => "◢",
        "volumeDown" => "◣",
        "volumeMute" | "volumeOff" => "◺",
        _ => "◆",
    }
}

struct Painter<'a> {
    view: &'a SurfaceView,
    /// The control the keyboard is on.
    focused: Option<usize>,
}

impl Painter<'_> {
    fn is_focused(&self, focus: Option<usize>) -> bool {
        focus.is_some() && focus == self.focused
    }

    /// `style` as the control at `focus` draws its label: bold and
    /// underlined under the keyboard, so focus reads without color and is
    /// never taken for a filled button.
    fn label(&self, focus: Option<usize>, style: Style) -> Style {
        if self.is_focused(focus) {
            style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            style
        }
    }

    /// The label of a checkbox or a choice.
    fn option_label(&self, focus: Option<usize>) -> Style {
        if self.is_focused(focus) {
            self.label(focus, Theme::accent())
        } else {
            Theme::text()
        }
    }

    /// The mark of a checkbox or a choice: its shape says whether it is
    /// selected, and the accent says so again.
    fn option_mark(
        &self,
        focus: Option<usize>,
        mark: &'static str,
        selected: bool,
    ) -> Span<'static> {
        let focused = self.is_focused(focus);
        let style = if selected || focused {
            Theme::accent()
        } else {
            Theme::dim()
        };
        Span::styled(
            mark,
            if focused {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            },
        )
    }

    fn editing(&self, focus: Option<usize>) -> Option<&Editing> {
        match &self.view.keyboard {
            Keyboard::On {
                editing: Some(editing),
                ..
            } if self.is_focused(focus) => Some(editing),
            Keyboard::On { .. } | Keyboard::Away { .. } => None,
        }
    }

    fn hint(&self) -> Option<&'static str> {
        match &self.view.keyboard {
            Keyboard::Away { reachable: false } => None,
            Keyboard::Away { reachable: true } => Some("Alt+U interact"),
            Keyboard::On {
                editing: Some(_), ..
            } => Some("Enter/Esc done"),
            Keyboard::On { index, .. } => {
                Some(match self.view.tree.focusables.get(*index)?.control {
                    Control::Field { .. } => "Tab move  Enter edit  Esc leave",
                    Control::Slider { .. } | Control::Tabs { .. } => {
                        "Tab move  Left/Right change  Esc leave"
                    }
                    Control::Button { .. }
                    | Control::Modal
                    | Control::CheckBox { .. }
                    | Control::Choice { .. } => "Tab move  Enter select  Esc leave",
                })
            }
        }
    }

    /// A card's content. A column that opens with a line of bold text has
    /// that line as its title.
    fn card(&self, content: &Node, width: usize) -> Vec<Line<'static>> {
        match content {
            Node::Column(children) => self.column(children, width, Titled::Yes),
            content => self.block(content, width),
        }
    }

    /// Children stacked, with a blank line between any two that do not
    /// belong together.
    fn column(&self, children: &[Node], width: usize, titled: Titled) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut above: Option<&Node> = None;
        for (index, child) in children.iter().enumerate() {
            let block = match child {
                Node::Text {
                    text,
                    role: TextRole::Body,
                } if titled == Titled::Yes && index == 0 && is_bold_line(text) => {
                    self.text(text, TextRole::Heading, width)
                }
                child => self.block(child, width),
            };
            if block.is_empty() {
                continue;
            }
            if above.is_some_and(|above| !sits_tight(above, child)) {
                lines.push(Line::default());
            }
            lines.extend(block);
            above = Some(child);
        }
        lines
    }

    fn text(&self, text: &str, role: TextRole, width: usize) -> Vec<Line<'static>> {
        let mut lines = render_markdown(text, width, &mut CodeFenceState::default()).lines;
        let style = match role {
            TextRole::Heading => Theme::input_prompt(),
            TextRole::Body => return lines,
            TextRole::Caption => Theme::dim(),
        };
        for span in lines.iter_mut().flat_map(|line| &mut line.spans) {
            span.style = span.style.patch(style);
        }
        lines
    }

    fn block(&self, node: &Node, width: usize) -> Vec<Line<'static>> {
        match node {
            Node::Text { text, role } => self.text(text, *role, width),
            Node::Icon(name) => vec![Line::from(Span::styled(icon_glyph(name), Theme::accent()))],
            Node::Media {
                kind,
                description,
                url,
            } => {
                let kind = match kind {
                    MediaKind::Image => "image",
                    MediaKind::Video => "video",
                    MediaKind::Audio => "audio",
                };
                let text = if description.is_empty() {
                    format!("▣ {kind}: {url}")
                } else {
                    format!("▣ {kind}: {description} ({url})")
                };
                wrapped(&text, width, Theme::dim())
            }
            Node::Divider => vec![Line::from(Span::styled("─".repeat(width), Theme::dim()))],
            Node::Row(children) => self.row(children, width),
            Node::Column(children) => self.column(children, width, Titled::No),
            Node::Card(content) => {
                framed(self.card(content, frame_inner(width)), width, Theme::dim())
            }
            Node::Tabs {
                focus,
                titles,
                active,
                body,
            } => {
                let mut bar = Vec::new();
                let mut rule = Vec::new();
                let mut used = 0;
                for (index, title) in titles.iter().enumerate() {
                    if index > 0 {
                        bar.push(Span::raw(" ".repeat(ROW_GAP)));
                        rule.push(Span::styled("─".repeat(ROW_GAP), Theme::dim()));
                        used += ROW_GAP;
                    }
                    let tab_width = display_width(title);
                    used += tab_width;
                    if index == *active {
                        bar.push(Span::styled(
                            title.clone(),
                            self.label(Some(*focus), Theme::input_prompt()),
                        ));
                        rule.push(Span::styled("━".repeat(tab_width), Theme::accent()));
                    } else {
                        bar.push(Span::styled(title.clone(), Theme::dim()));
                        rule.push(Span::styled("─".repeat(tab_width), Theme::dim()));
                    }
                }
                // The rule marks the active tab only while the bar is one
                // line; a bar that wraps leaves it to the tab's own style.
                let mut lines = if used <= width {
                    rule.push(Span::styled("─".repeat(width - used), Theme::dim()));
                    vec![Line::from(bar), Line::from(rule)]
                } else {
                    flow(bar, width)
                };
                lines.extend(body.iter().flat_map(|body| self.block(body, width)));
                lines
            }
            Node::Modal {
                focus,
                label,
                content,
            } => {
                let marker = if content.is_some() { "▾" } else { "▸" };
                let style = self.label(Some(*focus), Theme::accent());
                let mut lines = vec![Line::from(Span::styled(format!("{marker} {label}"), style))];
                if let Some(content) = content {
                    lines.extend(framed(
                        self.card(content, frame_inner(width)),
                        width,
                        Theme::dim(),
                    ));
                }
                lines
            }
            Node::Button {
                focus,
                label,
                variant,
                failures,
            } => {
                let enabled = matches!(
                    self.view
                        .tree
                        .focusables
                        .get(*focus)
                        .map(|focusable| &focusable.control),
                    Some(Control::Button { action: Some(_) })
                );
                let focus = Some(*focus);
                let fill = match variant {
                    ButtonVariant::Borderless => None,
                    ButtonVariant::Default | ButtonVariant::Primary if !enabled => {
                        Some(Theme::command_block().patch(Theme::activity_rail_dim()))
                    }
                    ButtonVariant::Default => Some(Theme::command_block()),
                    ButtonVariant::Primary => Some(Theme::accent_fill()),
                };
                let spans = match fill {
                    Some(fill) => {
                        let lead = if self.is_focused(focus) { "▸ " } else { "  " };
                        vec![
                            Span::styled(lead, fill),
                            Span::styled(label.clone(), self.label(focus, fill)),
                            Span::styled("  ", fill),
                        ]
                    }
                    None => {
                        let style = if enabled {
                            Theme::accent()
                        } else {
                            Theme::dim()
                        };
                        vec![Span::styled(label.clone(), self.label(focus, style))]
                    }
                };
                let mut lines = vec![Line::from(spans)];
                push_failures(&mut lines, failures, width);
                lines
            }
            Node::CheckBox {
                focus,
                label,
                checked,
                failures,
            } => {
                let mark = if *checked { "■" } else { "□" };
                let mut lines = hanging(
                    self.option_mark(*focus, mark, *checked),
                    label,
                    width,
                    self.option_label(*focus),
                );
                push_failures(&mut lines, failures, width);
                lines
            }
            Node::Field {
                focus,
                label,
                value,
                placeholder,
                kind,
                failures,
            } => {
                let mut lines = Vec::new();
                if !label.is_empty() {
                    lines.extend(wrapped(label, width, Theme::dim()));
                }
                let editing = self.editing(*focus);
                let fill = Theme::command_block();
                let edge = if self.is_focused(*focus) {
                    Span::styled("▌", fill.patch(Theme::accent()))
                } else {
                    Span::styled(" ", fill)
                };
                let (text, cursor) = match editing {
                    Some(editing) => (editing.text.as_str(), Some(editing.cursor)),
                    None => (value.as_str(), None),
                };
                // The value area: an edge, the text, and a column to spare.
                let inner = width.min(MAX_FIELD).saturating_sub(2).max(1);
                let rows = if text.is_empty() && cursor.is_none() {
                    let style = fill
                        .patch(Theme::activity_rail_dim())
                        .add_modifier(Modifier::ITALIC);
                    wrapped(placeholder, inner, style)
                        .into_iter()
                        .map(|line| line.spans)
                        .collect()
                } else {
                    field_rows(text, cursor, *kind, inner, fill)
                };
                for row in rows {
                    let pad = inner.saturating_sub(spans_display_width(&row)) + 1;
                    let mut spans = vec![edge.clone()];
                    spans.extend(row);
                    spans.push(Span::styled(" ".repeat(pad), fill));
                    lines.push(Line::from(spans));
                }
                if let Some(invalid) = editing.and_then(|editing| editing.invalid) {
                    lines.extend(wrapped(&format!("! {invalid}"), width, Theme::error()));
                }
                push_failures(&mut lines, failures, width);
                lines
            }
            Node::Choices {
                label,
                options,
                multiple,
                chips,
                failures,
            } => {
                let mut lines = Vec::new();
                if !label.is_empty() {
                    lines.extend(wrapped(label, width, Theme::dim()));
                }
                if *chips {
                    lines.extend(flow(self.chips(options), width));
                } else {
                    for option in options {
                        let mark = match (multiple, option.selected) {
                            (true, true) => "■",
                            (true, false) => "□",
                            (false, true) => "●",
                            (false, false) => "○",
                        };
                        lines.extend(hanging(
                            self.option_mark(option.focus, mark, option.selected),
                            &option.label,
                            width,
                            self.option_label(option.focus),
                        ));
                    }
                }
                push_failures(&mut lines, failures, width);
                lines
            }
            Node::Slider {
                focus,
                label,
                value,
                min,
                max,
                failures,
            } => {
                let mut lines = Vec::new();
                if !label.is_empty() {
                    lines.extend(wrapped(label, width, Theme::dim()));
                }
                let track = SLIDER_TRACK.min(width.saturating_sub(8)).max(1);
                let span = max - min;
                let filled = if span > 0.0 {
                    (((value - min) / span) * (track - 1) as f64).round() as usize
                } else {
                    0
                };
                let number = if self.is_focused(*focus) {
                    self.label(*focus, Theme::accent())
                } else {
                    Theme::text()
                };
                lines.push(Line::from(vec![
                    Span::styled("━".repeat(filled), Theme::accent()),
                    Span::styled("●", Theme::accent()),
                    Span::styled("─".repeat(track - 1 - filled), Theme::dim()),
                    Span::raw(" "),
                    Span::styled(number_text(*value), number),
                ]));
                push_failures(&mut lines, failures, width);
                lines
            }
            Node::Placeholder(kind) => wrapped(&format!("[{kind}]"), width, Theme::dim()),
        }
    }

    /// Children side by side at their natural widths when they fit, in
    /// equal columns when those are wide enough, and stacked otherwise.
    fn row(&self, children: &[Node], width: usize) -> Vec<Line<'static>> {
        let stacked: Vec<Vec<Line<'static>>> = children
            .iter()
            .map(|child| self.block(child, width))
            .collect();
        let gaps = ROW_GAP * children.len().saturating_sub(1);
        let natural: Vec<usize> = stacked.iter().map(|lines| block_width(lines)).collect();
        if natural.iter().sum::<usize>() + gaps <= width {
            return side_by_side(natural.into_iter().zip(stacked).collect());
        }
        let column = width.saturating_sub(gaps) / children.len().max(1);
        if column < MIN_COLUMN {
            return stacked.into_iter().flatten().collect();
        }
        side_by_side(
            children
                .iter()
                .map(|child| (column, self.block(child, column)))
                .collect(),
        )
    }

    fn chips(&self, options: &[ChoiceOption]) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for (index, option) in options.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" "));
            }
            let (text, style) = if option.selected {
                (format!("(✓ {})", option.label), Theme::accent_fill())
            } else {
                (format!("( {} )", option.label), Theme::text())
            };
            let style = if option.selected {
                self.label(option.focus, style)
            } else {
                self.option_label(option.focus)
            };
            spans.push(Span::styled(text, style));
        }
        spans
    }
}

/// Whether a column's first child is the title of the card it fills.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Titled {
    Yes,
    No,
}

/// Whether `text` is one line of markdown, all of it bold.
fn is_bold_line(text: &str) -> bool {
    let text = text.trim();
    text.strip_prefix("**")
        .and_then(|rest| rest.strip_suffix("**"))
        .is_some_and(|inner| !inner.is_empty() && !inner.contains("**") && !inner.contains('\n'))
}

/// Whether `below` follows `above` in a column with no blank line between:
/// a divider and its neighbors, lines of text, and a run of checkboxes.
fn sits_tight(above: &Node, below: &Node) -> bool {
    let prose = |node: &Node| {
        matches!(
            node,
            Node::Text { .. } | Node::Icon(_) | Node::Media { .. } | Node::Placeholder(_)
        )
    };
    let checkbox = |node: &Node| matches!(node, Node::CheckBox { .. });
    matches!(above, Node::Divider)
        || matches!(below, Node::Divider)
        || (prose(above) && prose(below))
        || (checkbox(above) && checkbox(below))
}

/// The width a frame `width` columns wide leaves its content.
fn frame_inner(width: usize) -> usize {
    if width < MIN_FRAMED {
        width
    } else {
        width - 4
    }
}

/// `content` in a rounded border `width` columns wide, or as it is where no
/// border fits.
fn framed(content: Vec<Line<'static>>, width: usize, border: Style) -> Vec<Line<'static>> {
    if width < MIN_FRAMED {
        return content;
    }
    let inner = frame_inner(width);
    let rule = "─".repeat(inner + 2);
    let mut lines = Vec::with_capacity(content.len() + 2);
    lines.push(Line::from(Span::styled(format!("╭{rule}╮"), border)));
    for line in content {
        let pad = inner.saturating_sub(spans_display_width(&line.spans));
        let mut spans = vec![Span::styled("│ ", border)];
        spans.extend(line.spans);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(" │", border));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(format!("╰{rule}╯"), border)));
    lines
}

fn block_width(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| spans_display_width(&line.spans))
        .max()
        .unwrap_or(0)
}

/// Columns of lines, each padded to its width, joined row by row.
fn side_by_side(columns: Vec<(usize, Vec<Line<'static>>)>) -> Vec<Line<'static>> {
    let height = columns
        .iter()
        .map(|(_, lines)| lines.len())
        .max()
        .unwrap_or(0);
    let last = columns.len().saturating_sub(1);
    let mut columns: Vec<_> = columns
        .into_iter()
        .map(|(width, lines)| (width, lines.into_iter()))
        .collect();
    (0..height)
        .map(|_| {
            let mut spans = Vec::new();
            for (index, (width, lines)) in columns.iter_mut().enumerate() {
                let cell = lines.next().map(|line| line.spans).unwrap_or_default();
                let used = spans_display_width(&cell);
                spans.extend(cell);
                if index < last {
                    spans.push(Span::raw(" ".repeat(width.saturating_sub(used) + ROW_GAP)));
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// `spans` on as many lines as `width` needs, breaking between spans.
fn flow(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut line: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        let span_width = display_width(&span.content);
        if used > 0 && used + span_width > width {
            lines.push(Line::from(std::mem::take(&mut line)));
            used = 0;
            if span.content.trim().is_empty() {
                continue;
            }
        }
        used += span_width;
        line.push(span);
    }
    if !line.is_empty() {
        lines.push(Line::from(line));
    }
    lines
}

fn wrapped(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_wrapped_text(&mut lines, text, width, style, LineFill::Natural);
    lines
}

/// `mark`, then `text` wrapped beside it with a hanging indent.
fn hanging(mark: Span<'static>, text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let indent = display_width(&mark.content) + 1;
    let mut mark = Some(mark);
    wrapped(text, width.saturating_sub(indent).max(1), style)
        .into_iter()
        .map(|line| {
            let mut spans = match mark.take() {
                Some(mark) => vec![mark, Span::raw(" ")],
                None => vec![Span::raw(" ".repeat(indent))],
            };
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

fn push_failures(lines: &mut Vec<Line<'static>>, failures: &[Failure], width: usize) {
    for failure in failures {
        let style = if failure.blocks {
            Theme::error()
        } else {
            Theme::warning()
        };
        lines.extend(wrapped(&format!("! {}", failure.message), width, style));
    }
}

/// A field's text on rows of at most `width` columns, breaking at its own
/// line ends and where a row is full, with the cell at `cursor` reversed.
fn field_rows(
    text: &str,
    cursor: Option<usize>,
    kind: FieldKind,
    width: usize,
    style: Style,
) -> Vec<Vec<Span<'static>>> {
    let caret = style.add_modifier(Modifier::REVERSED);
    let mut rows = vec![Vec::new()];
    let mut run = String::new();
    let mut used = 0;
    let length = text.chars().count();
    let flush = |rows: &mut Vec<Vec<Span<'static>>>, run: &mut String| {
        if !run.is_empty() {
            let row = rows.last_mut().expect("rows start with one row");
            row.push(Span::styled(std::mem::take(run), style));
        }
    };
    for (index, ch) in text.chars().chain(std::iter::once('\n')).enumerate() {
        let at_cursor = cursor == Some(index);
        let end = index == length;
        if ch == '\n' {
            flush(&mut rows, &mut run);
            if at_cursor {
                let row = rows.last_mut().expect("rows start with one row");
                row.push(Span::styled(" ", caret));
            }
            if !end {
                rows.push(Vec::new());
                used = 0;
            }
            continue;
        }
        let shown = if kind == FieldKind::Obscured {
            '•'
        } else {
            ch
        };
        let cell = shown.width().unwrap_or(0);
        if used + cell > width && used > 0 {
            flush(&mut rows, &mut run);
            rows.push(Vec::new());
            used = 0;
        }
        used += cell;
        if at_cursor {
            flush(&mut rows, &mut run);
            let row = rows.last_mut().expect("rows start with one row");
            row.push(Span::styled(shown.to_string(), caret));
        } else {
            run.push(shown);
        }
    }
    rows
}

/// A number as a person writes it: no fraction for a whole number.
pub(super) fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        let text = format!("{value:.2}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
