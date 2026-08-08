//! Syntect-backed syntax highlighting for fenced code blocks.
//!
//! Parses scopes only; no syntect theme is involved. Each scope region maps
//! onto a [`SyntaxRole`] which the theme resolves against the active ANSI
//! palette, so highlighting follows fixed, custom, and terminal-sampled
//! schemes alike.
//!
//! Language grammars come from [`two_face`]'s bat-derived dump (defaults plus
//! extras such as TypeScript and TOML), not syntect's smaller default set.

use std::sync::LazyLock;

use ratatui::style::Style;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

use super::{
    super::theme::{SyntaxRole, Theme},
    StyledSegment,
};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

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

/// Stateful highlighter for one fenced code block. Feed source lines in order.
pub(super) struct BlockHighlighter {
    parse: ParseState,
    stack: ScopeStack,
    /// Palette snapshot taken when the block opens. The history cache
    /// re-renders on theme generation changes, so styles cannot go stale
    /// mid-block.
    base: Style,
    selectors: Vec<(Scope, Style)>,
}

impl BlockHighlighter {
    /// Highlighter for a fence info token such as `rust`, `ts`, or `py`, or
    /// `None` when no bundled syntax matches (callers fall back to plain
    /// styling).
    pub(super) fn for_language(token: &str) -> Option<Self> {
        let syntax = SYNTAX_SET.find_syntax_by_token(canonical_language_token(token))?;
        let selectors = ROLE_SELECTORS
            .iter()
            .map(|(scope, role)| (*scope, Theme::markdown_syntax(*role)))
            .collect();
        Some(Self {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
            base: Theme::markdown_code_block(),
            selectors,
        })
    }

    /// Styled segments for one source line, without a trailing newline.
    pub(super) fn highlight_line(&mut self, line: &str) -> Vec<StyledSegment> {
        // Syntect grammars expect the newline to drive state transitions.
        let text = format!("{line}\n");
        let Ok(ops) = self.parse.parse_line(&text, &SYNTAX_SET) else {
            return vec![StyledSegment::new(line.to_string(), self.base)];
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
            segments.push(StyledSegment::new(String::new(), self.base));
        }
        segments
    }

    /// Style for the innermost scope that maps onto a role, else the base.
    fn scope_style(&self) -> Style {
        for scope in self.stack.as_slice().iter().rev() {
            for (selector, style) in &self.selectors {
                if selector.is_prefix_of(*scope) {
                    return *style;
                }
            }
        }
        self.base
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

/// Map common fence tags onto tokens the syntax dump actually registers.
///
/// Keep this short: prefer dump-native tokens (`ts`, `tsx`, `bash`) when the
/// author already used them. Only rewrite tags people type that still miss.
fn canonical_language_token(token: &str) -> &str {
    match token {
        "jsx" => "javascript",
        "shell" | "console" => "bash",
        other => other,
    }
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
