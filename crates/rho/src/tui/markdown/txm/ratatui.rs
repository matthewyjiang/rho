use ratatui_core::{
    buffer::Buffer,
    layout::{HorizontalAlignment, Rect, Size, VerticalAlignment},
    style::Style,
    widgets::Widget,
};
use unicode_width::UnicodeWidthChar;

use crate::tui::markdown::txm::backends::generic_backend::render_node;
use crate::tui::markdown::txm::backends::terminal::CharBuf;
use crate::tui::markdown::txm::layout_tree::LayoutNode;

#[derive(Debug, Clone)]
pub struct Math {
    mem: CharBuf,
    style: Style,
    horizontal_alignment: HorizontalAlignment,
    vertical_alignment: VerticalAlignment,
    display_width: u16,
}

fn compute_display_width(mem: &CharBuf) -> u16 {
    mem.data
        .chunks_exact(mem.width)
        .take(mem.height)
        .map(|row| {
            row.iter()
                .map(|&ch| UnicodeWidthChar::width(ch).unwrap_or(0) as u16)
                .sum()
        })
        .max()
        .unwrap_or(0)
}

impl Math {
    pub fn new(input: &str) -> Result<Self, crate::tui::markdown::txm::ParseError> {
        let tree = crate::tui::markdown::txm::layout(input)?;
        let mut mem = CharBuf::new(tree.width, tree.height);
        render_node(&tree, &mut mem, 0, 0);

        let display_width = compute_display_width(&mem);

        Ok(Self {
            mem,
            style: Style::default(),
            horizontal_alignment: HorizontalAlignment::Left,
            vertical_alignment: VerticalAlignment::Top,
            display_width,
        })
    }

    pub fn from_tree(tree: &LayoutNode) -> Self {
        let mut mem = CharBuf::new(tree.width, tree.height);
        render_node(tree, &mut mem, 0, 0);

        let display_width = compute_display_width(&mem);

        Self {
            mem,
            style: Style::default(),
            horizontal_alignment: HorizontalAlignment::Left,
            vertical_alignment: VerticalAlignment::Top,
            display_width,
        }
    }

    pub fn size(&self) -> Size {
        Rect::new(0, 0, self.display_width, self.mem.height as u16).as_size()
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn horizontal_alignment(mut self, alignment: HorizontalAlignment) -> Self {
        self.horizontal_alignment = alignment;
        self
    }

    pub fn vertical_alignment(mut self, alignment: VerticalAlignment) -> Self {
        self.vertical_alignment = alignment;
        self
    }
}

impl Widget for &Math {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let render_width = self.display_width;
        let render_height = self.mem.height as u16;
        if render_width == 0 || render_height == 0 {
            return;
        }

        for y in area.y..area.y.saturating_add(area.height) {
            for x in area.x..area.x.saturating_add(area.width) {
                buf[(x, y)].reset();
            }
        }

        let (content_x, draw_x) =
            align_horizontal_span(render_width, area.width, self.horizontal_alignment);
        let (content_y, draw_y, visible_height) =
            align_vertical_span(render_height, area.height, self.vertical_alignment);

        let visible_data_rows = self
            .mem
            .data
            .chunks_exact(self.mem.width)
            .skip(content_y as usize)
            .take(visible_height as usize);

        let visible_style_rows = self
            .mem
            .styles
            .chunks_exact(self.mem.width)
            .skip(content_y as usize)
            .take(visible_height as usize);

        let target_rows = draw_y as usize..draw_y as usize + visible_height as usize;

        for ((data_row, style_row), target_row) in
            visible_data_rows.zip(visible_style_rows).zip(target_rows)
        {
            let mut col = 0u16;
            let mut content_col = 0u16;
            let mut cell_buf = [0u8; 4];

            for (&ch, &cell_style) in data_row.iter().zip(style_row.iter()) {
                let cell_width = UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
                // Skip characters before content_x
                if content_col < content_x {
                    content_col += cell_width;
                    continue;
                }

                let x = area.x.saturating_add(draw_x).saturating_add(col);
                let y = area.y.saturating_add(target_row as u16);

                if x >= area.x + area.width {
                    break;
                }

                if x + cell_width > area.x + area.width {
                    break;
                }

                // Convert our style to ratatui's style
                // TODO(perf-rataui): map underline/dim/fg_color/bg_color from txm::Style
                let mut tui_style = ratatui_core::style::Style::default();
                if cell_style.is_bold() {
                    tui_style = tui_style.add_modifier(ratatui_core::style::Modifier::BOLD);
                }
                if cell_style.is_italic() {
                    tui_style = tui_style.add_modifier(ratatui_core::style::Modifier::ITALIC);
                }
                tui_style = self.style.patch(tui_style);

                let cell_str = ch.encode_utf8(&mut cell_buf);
                buf.set_stringn(x, y, cell_str, cell_width as usize, tui_style);

                col += cell_width;
                content_col += cell_width;
            }
        }
    }
}

fn align_horizontal_span(content: u16, area: u16, alignment: HorizontalAlignment) -> (u16, u16) {
    if content <= area {
        let draw = match alignment {
            HorizontalAlignment::Left => 0,
            HorizontalAlignment::Center => (area - content) / 2,
            HorizontalAlignment::Right => area - content,
        };
        (0, draw)
    } else {
        let content_start = match alignment {
            HorizontalAlignment::Left => 0,
            HorizontalAlignment::Center => (content - area) / 2,
            HorizontalAlignment::Right => content - area,
        };
        (content_start, 0)
    }
}

fn align_vertical_span(content: u16, area: u16, alignment: VerticalAlignment) -> (u16, u16, u16) {
    let visible = content.min(area);

    if content <= area {
        let draw = match alignment {
            VerticalAlignment::Top => 0,
            VerticalAlignment::Center => (area - content) / 2,
            VerticalAlignment::Bottom => area - content,
        };
        (0, draw, visible)
    } else {
        let content_start = match alignment {
            VerticalAlignment::Top => 0,
            VerticalAlignment::Center => (content - area) / 2,
            VerticalAlignment::Bottom => content - area,
        };
        (content_start, 0, visible)
    }
}

#[cfg(test)]
mod tests {
    use super::Math;
    use ratatui_core::{
        buffer::Buffer,
        layout::{HorizontalAlignment, Rect, VerticalAlignment},
        style::Style,
        widgets::Widget,
    };

    #[test]
    fn render_clears_short_rows_before_writing() {
        let tree = {
            use crate::tui::markdown::txm::layout_tree::{LayoutNode, NodeKind};
            use crate::tui::markdown::txm::style::Style as TxmStyle;
            LayoutNode {
                width: 2,
                height: 2,
                baseline: 0,
                style: TxmStyle::new(),
                kind: NodeKind::HStack {
                    children: vec![LayoutNode::from_char('a'), LayoutNode::from_char('b')],
                    spacing: 0,
                },
            }
        };

        let math = Math::from_tree(&tree);
        let area = Rect::new(0, 0, 2, 2);
        let mut buffer = Buffer::empty(area);
        for y in 0..area.height {
            for x in 0..area.width {
                buffer[(x, y)].set_symbol("x");
            }
        }

        (&math).render(area, &mut buffer);

        assert_eq!(buffer[(0, 0)].symbol(), "a");
        assert_eq!(buffer[(1, 0)].symbol(), "b");
    }

    #[test]
    fn render_clears_alignment_padding() {
        let tree = {
            use crate::tui::markdown::txm::layout_tree::{LayoutNode, NodeKind};
            use crate::tui::markdown::txm::style::Style as TxmStyle;
            LayoutNode {
                width: 1,
                height: 1,
                baseline: 0,
                style: TxmStyle::new(),
                kind: NodeKind::Text { content: vec!['a'] },
            }
        };

        let math = Math::from_tree(&tree)
            .style(Style::default())
            .horizontal_alignment(HorizontalAlignment::Center)
            .vertical_alignment(VerticalAlignment::Center);

        let area = Rect::new(0, 0, 3, 3);
        let mut buffer = Buffer::empty(area);
        for y in 0..area.height {
            for x in 0..area.width {
                buffer[(x, y)].set_symbol("x");
            }
        }

        (&math).render(area, &mut buffer);

        assert_eq!(buffer[(0, 0)].symbol(), " ");
        assert_eq!(buffer[(1, 1)].symbol(), "a");
        assert_eq!(buffer[(2, 2)].symbol(), " ");
    }
}
