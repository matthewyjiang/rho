//! Syntect-backed syntax highlighting for fenced code blocks.
//!
//! Parses scopes only; no syntect theme is involved. Each scope region maps
//! onto a [`SyntaxRole`] which the theme resolves against the active ANSI
//! palette, so highlighting follows fixed, custom, and terminal-sampled
//! schemes alike.

use std::sync::LazyLock;

use ratatui::style::Style;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use super::{
    super::theme::{SyntaxRole, Theme},
    StyledSegment,
};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);

/// Scope prefixes in match order: specific selectors before general ones.
static ROLE_SELECTORS: LazyLock<Vec<(Scope, SyntaxRole)>> = LazyLock::new(|| {
    [
        ("entity.name.function", SyntaxRole::Function),
        ("support.function", SyntaxRole::Function),
        ("entity.name.type", SyntaxRole::Type),
        ("support.type", SyntaxRole::Type),
        ("comment", SyntaxRole::Comment),
        ("string", SyntaxRole::String),
        ("constant", SyntaxRole::Constant),
        ("keyword", SyntaxRole::Keyword),
        ("storage", SyntaxRole::Keyword),
    ]
    .into_iter()
    .map(|(selector, role)| (Scope::new(selector).expect("valid scope selector"), role))
    .collect()
});

/// Palette snapshot taken when the block opens. The history cache re-renders
/// on theme generation changes, so styles cannot go stale mid-block.
struct SyntaxStyles {
    base: Style,
    comment: Style,
    string: Style,
    constant: Style,
    keyword: Style,
    function: Style,
    named_type: Style,
}

impl SyntaxStyles {
    fn current() -> Self {
        Self {
            base: Theme::markdown_code_block(),
            comment: Theme::markdown_syntax(SyntaxRole::Comment),
            string: Theme::markdown_syntax(SyntaxRole::String),
            constant: Theme::markdown_syntax(SyntaxRole::Constant),
            keyword: Theme::markdown_syntax(SyntaxRole::Keyword),
            function: Theme::markdown_syntax(SyntaxRole::Function),
            named_type: Theme::markdown_syntax(SyntaxRole::Type),
        }
    }

    fn style(&self, role: SyntaxRole) -> Style {
        match role {
            SyntaxRole::Comment => self.comment,
            SyntaxRole::String => self.string,
            SyntaxRole::Constant => self.constant,
            SyntaxRole::Keyword => self.keyword,
            SyntaxRole::Function => self.function,
            SyntaxRole::Type => self.named_type,
        }
    }
}

/// Stateful highlighter for one fenced code block. Feed source lines in order.
pub(super) struct BlockHighlighter {
    parse: ParseState,
    stack: ScopeStack,
    styles: SyntaxStyles,
}

impl BlockHighlighter {
    /// Highlighter for a fence info token such as `rust` or `py`, or `None`
    /// when no bundled syntax matches (callers fall back to plain styling).
    pub(super) fn for_language(token: &str) -> Option<Self> {
        let syntax = SYNTAX_SET.find_syntax_by_token(token)?;
        Some(Self {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
            styles: SyntaxStyles::current(),
        })
    }

    /// Styled segments for one source line, without a trailing newline.
    pub(super) fn highlight_line(&mut self, line: &str) -> Vec<StyledSegment> {
        // Syntect grammars expect the newline to drive state transitions.
        let text = format!("{line}\n");
        let Ok(ops) = self.parse.parse_line(&text, &SYNTAX_SET) else {
            return vec![StyledSegment::new(line.to_string(), self.styles.base)];
        };
        let mut segments: Vec<StyledSegment> = Vec::new();
        let mut cursor = 0usize;
        for (offset, op) in ops {
            let offset = offset.min(line.len());
            if offset > cursor {
                push_merged(&mut segments, &line[cursor..offset], self.scope_style());
                cursor = offset;
            }
            let _ = self.stack.apply(&op);
        }
        if cursor < line.len() {
            push_merged(&mut segments, &line[cursor..], self.scope_style());
        }
        if segments.is_empty() {
            segments.push(StyledSegment::new(String::new(), self.styles.base));
        }
        segments
    }

    /// Style for the innermost scope that maps onto a role, else the base.
    fn scope_style(&self) -> Style {
        for scope in self.stack.as_slice().iter().rev() {
            for (selector, role) in ROLE_SELECTORS.iter() {
                if selector.is_prefix_of(*scope) {
                    return self.styles.style(*role);
                }
            }
        }
        self.styles.base
    }
}

fn push_merged(segments: &mut Vec<StyledSegment>, text: &str, style: Style) {
    if let Some(last) = segments.last_mut() {
        if last.style == style {
            last.text.push_str(text);
            return;
        }
    }
    segments.push(StyledSegment::new(text.to_string(), style));
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
