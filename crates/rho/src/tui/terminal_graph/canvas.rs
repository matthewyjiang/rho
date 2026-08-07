// Adapted from Grok Build's terminal Mermaid renderer:
// https://github.com/xai-org/grok-build/blob/b189869b7755d2b482969acf6c92da3ecfeffd36/crates/codegen/xai-grok-markdown/src/mermaid.rs
// Copyright 2023-2026 SpaceXAI. Licensed under Apache-2.0.
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::painter::{GraphStyles, CONT};
pub(in crate::tui) const U: u8 = 1;
pub(in crate::tui) const D: u8 = 2;
pub(in crate::tui) const L: u8 = 4;
pub(in crate::tui) const R: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::tui) enum Cls {
    Empty,
    Border,
    Text,
    Edge,
    EdgeLabel,
    NodeBorder(usize),
    NodeText(usize),
}

pub(in crate::tui) const STY_DOT: u8 = 1;
pub(in crate::tui) const STY_THICK: u8 = 2;
pub(in crate::tui) const STY_SOLID: u8 = 4;

pub(in crate::tui) struct Canvas {
    pub(in crate::tui) w: usize,
    pub(in crate::tui) h: usize,
    pub(in crate::tui) ch: Vec<char>,
    suffix: Vec<String>,
    pub(in crate::tui) cls: Vec<Cls>,
    pub(in crate::tui) mask: Vec<u8>,
    pub(in crate::tui) style: Vec<u8>,
    pub(in crate::tui) occupied: Vec<bool>,
    pub(in crate::tui) cur_style: u8,
}

impl Canvas {
    pub(in crate::tui) fn new(w: usize, h: usize) -> Self {
        let n = w * h;
        Self {
            w,
            h,
            ch: vec![' '; n],
            suffix: vec![String::new(); n],
            cls: vec![Cls::Empty; n],
            mask: vec![0; n],
            style: vec![0; n],
            occupied: vec![false; n],
            cur_style: STY_SOLID,
        }
    }

    pub(in crate::tui) fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    pub(in crate::tui) fn set(&mut self, x: usize, y: usize, c: char, cls: Cls) {
        if x >= self.w || y >= self.h {
            return;
        }
        if c != CONT
            && c.width().unwrap_or(0) == 0
            && matches!(cls, Cls::Text | Cls::EdgeLabel | Cls::NodeText(_))
        {
            let mut previous = x.checked_sub(1);
            while let Some(px) = previous {
                let i = self.idx(px, y);
                if self.ch[i] != CONT {
                    self.suffix[i].push(c);
                    return;
                }
                previous = px.checked_sub(1);
            }
            return;
        }
        let i = self.idx(x, y);
        self.ch[i] = c;
        self.suffix[i].clear();
        self.cls[i] = cls;
    }

    pub(in crate::tui) fn set_grapheme(
        &mut self,
        x: usize,
        y: usize,
        grapheme: &str,
        cls: Cls,
    ) -> usize {
        let width = grapheme.width();
        if width == 0 {
            for character in grapheme.chars() {
                self.set(x, y, character, cls);
            }
            return 0;
        }
        let Some(first) = grapheme.chars().next() else {
            return 0;
        };
        if x >= self.w || y >= self.h || width > self.w - x {
            return width;
        }
        let index = self.idx(x, y);
        self.ch[index] = first;
        self.suffix[index].clear();
        self.suffix[index].push_str(&grapheme[first.len_utf8()..]);
        self.cls[index] = cls;
        for offset in 1..width {
            self.set(x + offset, y, CONT, cls);
        }
        width
    }

    pub(in crate::tui) fn add_bits(&mut self, x: usize, y: usize, bits: u8) {
        self.add_bits_with_class(x, y, bits, Cls::Edge);
    }

    pub(in crate::tui) fn add_bits_with_class(&mut self, x: usize, y: usize, bits: u8, class: Cls) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        if self.occupied[i] {
            return;
        }
        self.mask[i] |= bits;
        self.style[i] |= self.cur_style;
        if !is_border_class(self.cls[i]) {
            self.cls[i] = class;
        }
    }

    pub(in crate::tui) fn blit(&mut self, sub: &Canvas, ox: usize, oy: usize) {
        for sy in 0..sub.h {
            for sx in 0..sub.w {
                let (x, y) = (ox + sx, oy + sy);
                if x >= self.w || y >= self.h {
                    continue;
                }
                let si = sub.idx(sx, sy);
                let di = self.idx(x, y);
                self.ch[di] = sub.ch[si];
                self.suffix[di].clone_from(&sub.suffix[si]);
                self.cls[di] = sub.cls[si];
                self.mask[di] = sub.mask[si];
                self.style[di] = sub.style[si];
                self.occupied[di] = true;
            }
        }
    }

    pub(in crate::tui) fn junction(&mut self, x: usize, y: usize, bits: u8) {
        if x >= self.w || y >= self.h {
            return;
        }
        let i = self.idx(x, y);
        self.mask[i] |= bits;
        self.style[i] |= self.cur_style;
        if !is_border_class(self.cls[i]) {
            self.cls[i] = Cls::Edge;
        }
    }

    pub(in crate::tui) fn seg_v(&mut self, x: usize, y0: usize, y1: usize) {
        let (a, b) = (y0.min(y1), y0.max(y1));
        for y in a..=b {
            let mut bits = 0;
            if y > a {
                bits |= U;
            }
            if y < b {
                bits |= D;
            }
            self.add_bits(x, y, bits);
        }
    }

    pub(in crate::tui) fn seg_h(&mut self, y: usize, x0: usize, x1: usize) {
        let (a, b) = (x0.min(x1), x0.max(x1));
        for x in a..=b {
            let mut bits = 0;
            if x > a {
                bits |= L;
            }
            if x < b {
                bits |= R;
            }
            self.add_bits(x, y, bits);
        }
    }

    pub(in crate::tui) fn finalize_mask(&mut self) {
        for i in 0..self.ch.len() {
            if self.mask[i] != 0 && self.ch[i] == ' ' {
                let c = mask_char(self.mask[i]);
                self.ch[i] = match self.style[i] {
                    STY_DOT => dotted_char(c),
                    STY_THICK => thick_char(c),
                    _ => c,
                };
            }
        }
    }
    /// Mirror top-to-bottom for `BT` (rows reorder; within-row text is
    /// unaffected, so labels stay readable). Box-drawing glyphs flip too.
    pub(in crate::tui) fn flip_vertical(&mut self) {
        for y in 0..self.h / 2 {
            let y2 = self.h - 1 - y;
            for x in 0..self.w {
                let (i, j) = (self.idx(x, y), self.idx(x, y2));
                self.ch.swap(i, j);
                self.suffix.swap(i, j);
                self.cls.swap(i, j);
                self.mask.swap(i, j);
                self.style.swap(i, j);
                self.occupied.swap(i, j);
            }
        }
        for c in self.ch.iter_mut() {
            *c = flip_glyph_v(*c);
        }
    }

    /// Mirror left-to-right for `RL` while moving each text run as one unit.
    /// Cell order within a run is never reversed, so grapheme components and
    /// multi-codepoint emoji remain byte-for-byte in reading order.
    pub(in crate::tui) fn flip_horizontal(&mut self) {
        let mut text_runs = Vec::new();
        for y in 0..self.h {
            let mut x = 0;
            while x < self.w {
                let cls = self.cls[self.idx(x, y)];
                if matches!(cls, Cls::Text | Cls::EdgeLabel | Cls::NodeText(_)) {
                    let start = x;
                    while x < self.w && self.cls[self.idx(x, y)] == cls {
                        x += 1;
                    }
                    let end = x;
                    let cells = (start..end)
                        .map(|cell_x| {
                            let i = self.idx(cell_x, y);
                            (self.ch[i], std::mem::take(&mut self.suffix[i]))
                        })
                        .collect::<Vec<_>>();
                    for cell_x in start..end {
                        let i = self.idx(cell_x, y);
                        self.ch[i] = ' ';
                        self.cls[i] = Cls::Empty;
                    }
                    text_runs.push((y, start, end, cls, cells));
                } else {
                    x += 1;
                }
            }
        }

        for y in 0..self.h {
            for x in 0..self.w / 2 {
                let x2 = self.w - 1 - x;
                let (i, j) = (self.idx(x, y), self.idx(x2, y));
                self.ch.swap(i, j);
                self.suffix.swap(i, j);
                self.cls.swap(i, j);
                self.mask.swap(i, j);
                self.style.swap(i, j);
                self.occupied.swap(i, j);
            }
        }
        for c in self.ch.iter_mut() {
            *c = flip_glyph_h(*c);
        }
        for (y, _start, end, cls, cells) in text_runs {
            let target = self.w - end;
            for (offset, (character, suffix)) in cells.into_iter().enumerate() {
                let i = self.idx(target + offset, y);
                self.ch[i] = character;
                self.suffix[i] = suffix;
                self.cls[i] = cls;
            }
        }
    }

    pub(in crate::tui) fn to_lines(
        &self,
        styles: &GraphStyles,
    ) -> (Vec<Line<'static>>, Vec<String>) {
        let mut styled = Vec::with_capacity(self.h);
        let mut plain = Vec::with_capacity(self.h);
        for y in 0..self.h {
            let mut last = self.w;
            for x in (0..self.w).rev() {
                let c = self.ch[self.idx(x, y)];
                if c != ' ' && c != CONT {
                    last = x + 1;
                    break;
                }
            }
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut plain_row = String::new();
            let mut run = String::new();
            let mut run_cls = Cls::Empty;
            for x in 0..last {
                let i = self.idx(x, y);
                let c = self.ch[i];
                if c == CONT {
                    continue;
                }
                let cls = self.cls[i];
                plain_row.push(c);
                plain_row.push_str(&self.suffix[i]);
                if cls != run_cls && !run.is_empty() {
                    spans.push(Span::styled(
                        std::mem::take(&mut run),
                        style_for(run_cls, styles),
                    ));
                }
                run_cls = cls;
                run.push(c);
                run.push_str(&self.suffix[i]);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, style_for(run_cls, styles)));
            }
            styled.push(Line::from(spans));
            plain.push(plain_row.trim_end().to_string());
        }
        (styled, plain)
    }
}

fn style_for(cls: Cls, styles: &GraphStyles) -> Style {
    match cls {
        Cls::Empty => Style::default(),
        Cls::Border => styles.border,
        Cls::Text => styles.node_text,
        Cls::Edge => styles.edge,
        Cls::EdgeLabel => styles.edge_label,
        Cls::NodeBorder(index) => styles.node_style(index).border,
        Cls::NodeText(index) => styles.node_style(index).text,
    }
}

fn is_border_class(cls: Cls) -> bool {
    matches!(cls, Cls::Border | Cls::NodeBorder(_))
}

fn mask_char(mask: u8) -> char {
    match mask {
        0 => ' ',
        m if m == U || m == D || m == U | D => '│',
        m if m == L || m == R || m == L | R => '─',
        m if m == D | R => '┌',
        m if m == D | L => '┐',
        m if m == U | R => '└',
        m if m == U | L => '┘',
        m if m == U | D | R => '├',
        m if m == U | D | L => '┤',
        m if m == D | L | R => '┬',
        m if m == U | L | R => '┴',
        _ => '┼',
    }
}

fn dotted_char(c: char) -> char {
    match c {
        '─' => '╌',
        '│' => '╎',
        other => other,
    }
}

fn thick_char(c: char) -> char {
    match c {
        '─' => '━',
        '│' => '┃',
        '┌' => '┏',
        '┐' => '┓',
        '└' => '┗',
        '┘' => '┛',
        '├' => '┣',
        '┤' => '┫',
        '┬' => '┳',
        '┴' => '┻',
        '┼' => '╋',
        other => other,
    }
}

fn flip_glyph_v(c: char) -> char {
    match c {
        '┌' => '└',
        '└' => '┌',
        '┐' => '┘',
        '┘' => '┐',
        '┏' => '┗',
        '┗' => '┏',
        '┓' => '┛',
        '┛' => '┓',
        '╭' => '╰',
        '╰' => '╭',
        '╮' => '╯',
        '╯' => '╮',
        '┬' => '┴',
        '┴' => '┬',
        '┳' => '┻',
        '┻' => '┳',
        '▼' => '▲',
        '▲' => '▼',
        '▽' => '△',
        '△' => '▽',
        other => other,
    }
}

fn flip_glyph_h(c: char) -> char {
    match c {
        '┌' => '┐',
        '┐' => '┌',
        '└' => '┘',
        '┘' => '└',
        '┏' => '┓',
        '┓' => '┏',
        '┗' => '┛',
        '┛' => '┗',
        '╭' => '╮',
        '╮' => '╭',
        '╰' => '╯',
        '╯' => '╰',
        '├' => '┤',
        '┤' => '├',
        '┣' => '┫',
        '┫' => '┣',
        '▶' => '◄',
        '◄' => '▶',
        '▷' => '◁',
        '◁' => '▷',
        other => other,
    }
}
