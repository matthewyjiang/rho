//! Vendored copy of txm, a terminal math rendering engine.
//!
//! Source: <https://github.com/thatmagicalcat/txm> at commit
//! 26e6d3cfef4d5d040711f4c49d9147aabd350d4d (MIT OR Apache-2.0, see
//! THIRD-PARTY-NOTICES). crates.io txm 0.1.5 ships a broken `ratatui`
//! feature and cargo cannot publish a crate with a git dependency, so the
//! library sources live here until upstream ships a fixed release.
//! TODO: replace this module with the crates.io txm dependency once a
//! release newer than 0.1.5 builds with the `ratatui` feature.
//!
//! Local changes only: `crate::` paths rewritten to this module's path, the
//! `ratatui` feature gate removed, let chains ported to edition 2021, lint
//! allows added, and rustfmt applied.

#![allow(dead_code, unused_imports, clippy::all)]

use crate::tui::markdown::txm::backend::Backend;
use crate::tui::markdown::txm::glyph::*;
use crate::tui::markdown::txm::parser::Parser;
use crate::tui::markdown::txm::render::render as render_expr;
use crate::tui::markdown::txm::token::tokenize;

use std::sync::OnceLock;

mod ast;
mod error;
mod glyph;
mod layout_tree;
mod parser;
mod render;
mod style;
mod token;

pub use error::ParseError;
pub use layout_tree::{LayoutNode, LineStyle, NodeKind};
pub use style::Style;

pub mod backend;
pub mod backends;

pub mod ratatui;

const COMPACT_SIMPLE_FRACTIONAL_EXPONENTS: bool = false;

/// Renders a math expression to a `LayoutNode` tree.
pub fn layout(input: &str) -> Result<LayoutNode, ParseError> {
    let tokens = tokenize(input)?;
    let reg = registry();
    let mut parser = Parser::new(input, &tokens, reg);
    let expr = parser.parse_expr()?;
    let mut ctx = RenderCtx {
        depth: 0,
        current_style: Style::new().italic(),
    };
    render_expr(&expr, reg, &mut ctx)
}

/// Renders a math expression to a plain text string with ANSI styling.
pub fn render(input: &str) -> Result<String, ParseError> {
    let tree = layout(input)?;
    let backend = backends::terminal::TerminalBackend::new();
    Ok(backend.render(&tree).unwrap())
}

fn registry() -> &'static SymbolRegistry {
    static REGISTRY: OnceLock<SymbolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(build_registry)
}

fn build_registry() -> SymbolRegistry {
    let mut r = SymbolRegistry::new();

    for (cmd, ch) in [
        ("alpha", 'α'),
        ("beta", 'β'),
        ("gamma", 'γ'),
        ("delta", 'δ'),
        ("epsilon", 'ε'),
        ("zeta", 'ζ'),
        ("eta", 'η'),
        ("theta", 'θ'),
        ("iota", 'ι'),
        ("kappa", 'κ'),
        ("lambda", 'λ'),
        ("mu", 'μ'),
        ("nu", 'ν'),
        ("xi", 'ξ'),
        ("omicron", 'ο'),
        ("pi", 'π'),
        ("rho", 'ρ'),
        ("sigma", 'σ'),
        ("tau", 'τ'),
        ("upsilon", 'υ'),
        ("phi", 'φ'),
        ("chi", 'χ'),
        ("psi", 'ψ'),
        ("omega", 'ω'),
        ("vee", '∨'),
        ("wedge", '∧'),
    ] {
        r.register(cmd, UnicodeGlyph(ch));
    }

    for (cmd, ch) in [
        ("Gamma", 'Γ'),
        ("Delta", 'Δ'),
        ("Theta", 'Θ'),
        ("Lambda", 'Λ'),
        ("Xi", 'Ξ'),
        ("Pi", 'Π'),
        ("Sigma", 'Σ'),
        ("Phi", 'Φ'),
        ("Psi", 'Ψ'),
        ("Omega", 'Ω'),
    ] {
        r.register(cmd, UnicodeGlyph(ch));
    }

    for name in &[
        "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh",
        "coth", "tanh", "log", "ln", "lg", "det", "dim", "hom", "ker", "exp", "deg", "gcd", "lcm",
        "sup", "inf", "max", "min", "arg", "Pr", "mod", "adj",
    ] {
        r.register(*name, TextGlyph(name));
    }

    r.register("binom", BinomGlyph);
    r.register("frac", FracGlyph);
    r.register("sqrt", SqrtGlyph);
    r.register("lim", LimitGlyph);
    r.register("int", IntegralGlyph);
    r.register("sum", SummationGlyph);
    r.register("prod", ProductGlyph);

    r.register("color", TextColorGlyph);

    for (cmd, ch) in [
        ("infty", '∞'),
        ("partial", '∂'),
        ("nabla", '∇'),
        ("forall", '∀'),
        ("exists", '∃'),
        ("neg", '¬'),
        ("emptyset", '∅'),
        ("triangle", '△'),
        ("angle", '∠'),
        ("therefore", '∴'),
        ("because", '∵'),
        ("cdot", '·'),
        ("times", '×'),
        ("div", '÷'),
        ("pm", '±'),
        ("mp", '∓'),
        ("circ", '∘'),
        ("bullet", '∙'),
        ("star", '⋆'),
        ("le", '≤'),
        ("ge", '≥'),
        ("ne", '≠'),
        ("approx", '≈'),
        ("equiv", '≡'),
        ("sim", '∼'),
        ("simeq", '≃'),
        ("cong", '≅'),
        ("propto", '∝'),
        ("perp", '⊥'),
        ("parallel", '∥'),
        ("to", '→'),
        ("rightarrow", '→'),
        ("Rightarrow", '⇒'),
        ("leftarrow", '←'),
        ("Leftarrow", '⇐'),
        ("mapsto", '↦'),
        ("implies", '⇒'),
        ("iff", '⇔'),
        ("in", '∈'),
        ("notin", '∉'),
        ("subset", '⊂'),
        ("supset", '⊃'),
        ("subseteq", '⊆'),
        ("supseteq", '⊇'),
        ("cup", '∪'),
        ("cap", '∩'),
        ("lvert", '|'),
        ("rvert", '|'),
        ("langle", '⟨'),
        ("rangle", '⟩'),
        ("lfloor", '⌊'),
        ("rfloor", '⌋'),
        ("lceil", '⌈'),
        ("rceil", '⌉'),
        ("quad", ' '),
        ("dots", '⋯'),
        ("ldots", '…'),
        ("vdots", '⋮'),
        ("ddots", '⋱'),
        ("aleph", 'ℵ'),
        ("hbar", 'ℏ'),
        ("ell", 'ℓ'),
        ("wp", '℘'),
        ("oplus", '⊕'),
        ("ominus", '⊖'),
        ("otimes", '⊗'),
        ("oslash", '⊘'),
    ] {
        r.register(cmd, UnicodeGlyph(ch));
    }

    r.register("abs", AbsGlyph);
    r.register("|", AbsGlyph);

    r.register("mathbb", AlphabetGlyph(to_bb as fn(char) -> char));

    for (cmd, modify, map) in [
        (
            "mathbf",
            (|s| s.un_italic().bold()) as fn(Style) -> Style,
            to_bold as fn(char) -> char,
        ),
        ("mathrm", Style::un_italic, to_upright),
        ("mathsf", Style::un_italic, to_sans),
        ("mathit", Style::italic, to_italic),
        ("boldsymbol", (|s| s.bold()) as fn(Style) -> Style, to_bold),
    ] {
        r.register(cmd, MappedStyleGlyph { modify, map });
    }

    for (cmd, modify) in [
        ("text", Style::un_italic as fn(Style) -> Style),
        ("textbf", (|s| s.un_italic().bold()) as fn(Style) -> Style),
        ("textit", Style::italic as fn(Style) -> Style),
        ("textrm", Style::un_italic as fn(Style) -> Style),
    ] {
        r.register(cmd, StyleModifierGlyph { modify });
    }

    r.register("colorbox", BgColorGlyph);

    for (cmd, mark) in [
        ("hat", '^'),
        ("tilde", '~'),
        ("bar", '‾'),
        ("vec", '→'),
        ("dot", '˙'),
        ("ddot", '¨'),
        ("acute", '´'),
        ("grave", '`'),
        ("check", 'ˇ'),
        ("breve", '˘'),
    ] {
        r.register(
            cmd,
            AccentGlyph {
                mark,
                stretch: false,
            },
        );
    }

    for (cmd, mark) in [("overline", '─'), ("widehat", '^'), ("widetilde", '~')] {
        r.register(
            cmd,
            AccentGlyph {
                mark,
                stretch: true,
            },
        );
    }

    r
}
