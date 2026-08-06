use crate::tui::markdown::txm::ast::Expr;
use crate::tui::markdown::txm::layout_tree::{LayoutNode, NodeKind};
use crate::tui::markdown::txm::style::Style;
use crate::tui::markdown::txm::ParseError;
use std::collections::HashMap;
use std::fmt::Debug;

#[derive(Debug, Default, Clone, Copy)]
pub struct RenderCtx {
    pub depth: usize,
    pub current_style: Style,
}

pub trait Glyph: Debug + Send + Sync {
    fn required_args(&self) -> usize {
        0
    }

    fn has_optional(&self) -> bool {
        false
    }

    fn has_limits(&self) -> bool {
        false
    }

    fn takes_string_arg(&self) -> bool {
        false
    }

    fn render_macro(
        &self,
        args: &[Expr],
        opts: &[Expr],
        ctx: &mut RenderCtx,
        eval: &mut dyn FnMut(&Expr, &mut RenderCtx) -> Result<LayoutNode, ParseError>,
    ) -> Result<LayoutNode, ParseError> {
        let mut rendered_args = Vec::with_capacity(args.len());
        for arg in args {
            rendered_args.push(eval(arg, ctx)?);
        }

        let mut rendered_opts = Vec::with_capacity(opts.len());
        for opt in opts {
            rendered_opts.push(eval(opt, ctx)?);
        }

        Ok(self.render(&rendered_args, &rendered_opts, ctx))
    }

    fn render(&self, _args: &[LayoutNode], _opts: &[LayoutNode], _ctx: &RenderCtx) -> LayoutNode {
        LayoutNode::empty()
    }
}

pub struct SymbolRegistry {
    map: HashMap<String, Box<dyn Glyph>>,
}

impl SymbolRegistry {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn register(&mut self, name: impl Into<String>, glyph: impl Glyph + 'static) {
        self.map.insert(name.into(), Box::new(glyph));
    }

    pub fn get(&self, name: &str) -> Option<&dyn Glyph> {
        self.map.get(name).map(|g| g.as_ref())
    }
}

#[derive(Debug)]
pub struct LimitGlyph;

impl Glyph for LimitGlyph {
    fn render(&self, _args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let mut node = LayoutNode::text_str("lim");
        node.style = ctx.current_style.un_italic();
        node
    }

    fn required_args(&self) -> usize {
        0
    }

    fn has_limits(&self) -> bool {
        true
    }
}

#[derive(Debug)]
pub struct UnicodeGlyph(pub char);

impl Glyph for UnicodeGlyph {
    fn render(&self, _args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        LayoutNode::text_with_style(vec![self.0], ctx.current_style.un_italic())
    }
}

#[derive(Debug)]
pub struct TextGlyph(pub &'static str);

impl Glyph for TextGlyph {
    fn render(&self, _args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        LayoutNode::text_with_style(self.0.chars().collect(), ctx.current_style.un_italic())
    }
}

#[derive(Debug)]
pub struct BinomGlyph;

impl Glyph for BinomGlyph {
    fn required_args(&self) -> usize {
        2
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let inner = LayoutNode::vstack(
            args[0].clone(),
            args[1].clone(),
            crate::tui::markdown::txm::layout_tree::LineStyle::None,
            ctx.current_style,
        );

        LayoutNode::stretchy_delim(inner, '(', ')', false, ctx.current_style.un_italic())
    }
}

#[derive(Debug)]
pub struct FracGlyph;

impl Glyph for FracGlyph {
    fn required_args(&self) -> usize {
        2
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        LayoutNode::vstack(
            args[0].clone(),
            args[1].clone(),
            crate::tui::markdown::txm::layout_tree::LineStyle::Solid,
            ctx.current_style,
        )
    }
}

#[derive(Debug)]
pub struct SqrtGlyph;

impl Glyph for SqrtGlyph {
    fn required_args(&self) -> usize {
        1
    }

    fn has_optional(&self) -> bool {
        true
    }

    fn render(&self, args: &[LayoutNode], opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let index = opts.first().cloned();
        let mut node = LayoutNode::sqrt(args[0].clone(), index);
        node.style = ctx.current_style;
        node
    }
}

#[derive(Debug)]
pub struct SummationGlyph;
impl Glyph for SummationGlyph {
    fn has_optional(&self) -> bool {
        true
    }

    fn has_limits(&self) -> bool {
        true
    }

    fn required_args(&self) -> usize {
        1
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let inner = if args.is_empty() {
            None
        } else {
            Some(args[0].clone())
        };
        let mut node = LayoutNode::summation(inner);
        node.style = ctx.current_style;
        node
    }
}

#[derive(Debug)]
pub struct ProductGlyph;
impl Glyph for ProductGlyph {
    fn has_limits(&self) -> bool {
        true
    }

    fn required_args(&self) -> usize {
        1
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let inner = if args.is_empty() {
            None
        } else {
            Some(args[0].clone())
        };
        let mut node = LayoutNode::product(inner);
        node.style = ctx.current_style;
        node
    }
}

#[derive(Debug)]
pub struct IntegralGlyph;

impl Glyph for IntegralGlyph {
    fn has_limits(&self) -> bool {
        true
    }

    fn required_args(&self) -> usize {
        1
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let inner = if args.is_empty() {
            None
        } else {
            Some(args[0].clone())
        };

        let mut node = LayoutNode::integral(inner);
        node.style = ctx.current_style;
        node
    }
}

#[derive(Debug)]
pub struct AlphabetGlyph(pub fn(char) -> char);

impl Glyph for AlphabetGlyph {
    fn required_args(&self) -> usize {
        1
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let src = &args[0];
        match &src.kind {
            crate::tui::markdown::txm::layout_tree::NodeKind::Text { content } => {
                let mapped: Vec<char> = content.iter().map(|&c| (self.0)(c)).collect();
                LayoutNode {
                    width: src.width,
                    height: src.height,
                    baseline: src.baseline,
                    style: ctx.current_style.merge(src.style),
                    kind: crate::tui::markdown::txm::layout_tree::NodeKind::Text {
                        content: mapped,
                    },
                }
            }
            _ => {
                let mut node = src.clone();
                node.style = ctx.current_style.merge(node.style);
                node
            }
        }
    }
}

fn shift(c: char, base: u32, off: u32) -> char {
    char::from_u32(base + off).unwrap_or(c)
}

pub fn to_bold(c: char) -> char {
    match c {
        'A'..='Z' => shift(c, 0x1D400, c as u32 - 'A' as u32),
        'a'..='z' => shift(c, 0x1D41A, c as u32 - 'a' as u32),
        '0'..='9' => shift(c, 0x1D7CE, c as u32 - '0' as u32),
        // Italic uppercase → bold italic uppercase
        '\u{1D434}'..='\u{1D44D}' => shift(c, 0x1D468, c as u32 - 0x1D434),
        // Italic lowercase → bold italic lowercase
        '\u{1D44E}'..='\u{1D467}' => shift(c, 0x1D482, c as u32 - 0x1D44E),
        _ => c,
    }
}

pub fn to_bb(c: char) -> char {
    match c {
        'C' => 'ℂ',
        'H' => 'ℍ',
        'N' => 'ℕ',
        'P' => 'ℙ',
        'Q' => 'ℚ',
        'R' => 'ℝ',
        'Z' => 'ℤ',
        'A'..='Z' => shift(c, 0x1D538, c as u32 - 'A' as u32),
        'a'..='z' => shift(c, 0x1D552, c as u32 - 'a' as u32),
        '0'..='9' => shift(c, 0x1D7D8, c as u32 - '0' as u32),
        // Italic uppercase → double-struck (with special cases for common letters)
        '\u{1D436}' => 'ℂ', // italic C
        '\u{1D43B}' => 'ℍ', // italic H
        '\u{1D441}' => 'ℕ', // italic N
        '\u{1D443}' => 'ℙ', // italic P
        '\u{1D444}' => 'ℚ', // italic Q
        '\u{1D445}' => 'ℝ', // italic R
        '\u{1D44D}' => 'ℤ', // italic Z
        '\u{1D434}'..='\u{1D44D}' => shift(c, 0x1D538, c as u32 - 0x1D434),
        // Italic lowercase → double-struck lowercase
        '\u{1D44E}'..='\u{1D467}' => shift(c, 0x1D552, c as u32 - 0x1D44E),
        _ => c,
    }
}

pub fn to_upright(c: char) -> char {
    c
}

pub fn to_italic(c: char) -> char {
    match c {
        'h' => 'ℎ',
        'A'..='Z' => shift(c, 0x1D434, c as u32 - 'A' as u32),
        'a'..='z' => shift(c, 0x1D44E, c as u32 - 'a' as u32),
        _ => c,
    }
}

#[allow(dead_code)]
pub fn to_bold_italic(c: char) -> char {
    match c {
        'A'..='Z' => shift(c, 0x1D468, c as u32 - 'A' as u32),
        'a'..='z' => shift(c, 0x1D482, c as u32 - 'a' as u32),
        // Italic uppercase → bold italic uppercase
        '\u{1D434}'..='\u{1D44D}' => shift(c, 0x1D468, c as u32 - 0x1D434),
        // Italic lowercase → bold italic lowercase
        '\u{1D44E}'..='\u{1D467}' => shift(c, 0x1D482, c as u32 - 0x1D44E),
        _ => c,
    }
}

pub fn to_sans(c: char) -> char {
    match c {
        'A'..='Z' => shift(c, 0x1D5A0, c as u32 - 'A' as u32),
        'a'..='z' => shift(c, 0x1D5BA, c as u32 - 'a' as u32),
        '0'..='9' => shift(c, 0x1D7E2, c as u32 - '0' as u32),
        // Italic uppercase → sans-serif italic uppercase
        '\u{1D434}'..='\u{1D44D}' => shift(c, 0x1D5A0 + 0x60, c as u32 - 0x1D434),
        // Italic lowercase → sans-serif italic lowercase
        '\u{1D44E}'..='\u{1D467}' => shift(c, 0x1D5BA + 0x60, c as u32 - 0x1D44E),
        _ => c,
    }
}

#[derive(Debug)]
pub struct AbsGlyph;

impl Glyph for AbsGlyph {
    fn required_args(&self) -> usize {
        1
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let mut node = LayoutNode::stretchy_delim(
            args[0].clone(),
            '|',
            '|',
            false,
            ctx.current_style.un_italic(),
        );

        node.style = ctx.current_style;
        node
    }
}

#[derive(Debug)]
pub struct AccentGlyph {
    pub mark: char,
    pub stretch: bool,
}

impl Glyph for AccentGlyph {
    fn required_args(&self) -> usize {
        1
    }

    fn render(&self, args: &[LayoutNode], _opts: &[LayoutNode], ctx: &RenderCtx) -> LayoutNode {
        let mut node = LayoutNode::accent(args[0].clone(), self.mark, self.stretch);
        node.style = ctx.current_style.un_italic();
        node
    }
}

#[derive(Debug)]
pub struct TextColorGlyph;

impl Glyph for TextColorGlyph {
    fn required_args(&self) -> usize {
        2
    }

    fn takes_string_arg(&self) -> bool {
        true
    }

    fn render_macro(
        &self,
        args: &[Expr],
        _opts: &[Expr],
        ctx: &mut RenderCtx,
        eval: &mut dyn FnMut(&Expr, &mut RenderCtx) -> Result<LayoutNode, ParseError>,
    ) -> Result<LayoutNode, ParseError> {
        let color_str = if let Expr::Ident(c) = &args[0] {
            c.as_str()
        } else {
            return Err(ParseError::ExpectedColorName);
        };

        let prev_style = ctx.current_style;
        ctx.current_style = ctx
            .current_style
            .fg(crate::tui::markdown::txm::style::parse_color(color_str)?);

        let result = eval(&args[1], ctx);

        ctx.current_style = prev_style;
        result
    }
}

#[derive(Debug)]
pub struct StyleModifierGlyph {
    pub modify: fn(Style) -> Style,
}

impl Glyph for StyleModifierGlyph {
    fn required_args(&self) -> usize {
        1
    }

    fn takes_string_arg(&self) -> bool {
        true
    }

    fn render_macro(
        &self,
        args: &[Expr],
        _opts: &[Expr],
        ctx: &mut RenderCtx,
        eval: &mut dyn FnMut(&Expr, &mut RenderCtx) -> Result<LayoutNode, ParseError>,
    ) -> Result<LayoutNode, ParseError> {
        let prev = ctx.current_style;
        ctx.current_style = (self.modify)(ctx.current_style);
        let result = eval(&args[0], ctx);
        ctx.current_style = prev;
        result
    }
}

#[derive(Debug)]
pub struct MappedStyleGlyph {
    pub modify: fn(Style) -> Style,
    pub map: fn(char) -> char,
}

impl Glyph for MappedStyleGlyph {
    fn required_args(&self) -> usize {
        1
    }

    fn render_macro(
        &self,
        args: &[Expr],
        _opts: &[Expr],
        ctx: &mut RenderCtx,
        eval: &mut dyn FnMut(&Expr, &mut RenderCtx) -> Result<LayoutNode, ParseError>,
    ) -> Result<LayoutNode, ParseError> {
        let prev = ctx.current_style;
        ctx.current_style = (self.modify)(ctx.current_style);
        let mut result = eval(&args[0], ctx)?;
        ctx.current_style = prev;
        map_node_chars(&mut result, self.map);
        Ok(result)
    }
}

fn map_node_chars(node: &mut LayoutNode, map: fn(char) -> char) {
    match &mut node.kind {
        NodeKind::Text { content } => {
            for c in content.iter_mut() {
                *c = map(*c);
            }
        }
        NodeKind::HStack { children, .. } => {
            for child in children.iter_mut() {
                map_node_chars(child, map);
            }
        }
        _ => {}
    }
}

#[derive(Debug)]
pub struct BgColorGlyph;

impl Glyph for BgColorGlyph {
    fn required_args(&self) -> usize {
        2
    }

    fn takes_string_arg(&self) -> bool {
        true
    }

    fn render_macro(
        &self,
        args: &[Expr],
        _opts: &[Expr],
        ctx: &mut RenderCtx,
        eval: &mut dyn FnMut(&Expr, &mut RenderCtx) -> Result<LayoutNode, ParseError>,
    ) -> Result<LayoutNode, ParseError> {
        let color_str = if let Expr::Ident(c) = &args[0] {
            c.as_str()
        } else {
            return Err(ParseError::ExpectedColorName);
        };

        let prev = ctx.current_style;
        ctx.current_style = ctx
            .current_style
            .bg(crate::tui::markdown::txm::style::parse_color(color_str)?);
        let result = eval(&args[1], ctx);
        ctx.current_style = prev;
        result
    }
}
