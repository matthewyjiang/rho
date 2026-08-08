//! Syntect-backed syntax highlighting for code fences and diff bodies.
//!
//! Parses scopes only; no syntect theme is involved. Each scope region maps
//! onto a [`SyntaxRole`] (or plain). Callers resolve roles against the active
//! palette so highlighting follows fixed, custom, and terminal-sampled themes
//! and so diff add/remove colors can supply their own plain style.
//!
//! Language grammars come from [`two_face`]'s bat-derived dump (defaults plus
//! extras such as TypeScript and TOML), not syntect's smaller default set.

use std::{path::Path, sync::LazyLock};

use ratatui::style::Style;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use super::theme::{SyntaxRole, Theme};

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

/// One highlighted span: plain (no role) or a syntax role. Callers map plain to
/// their base style (body text, add/remove color, …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct HighlightSegment {
    pub(in crate::tui) text: String,
    pub(in crate::tui) role: Option<SyntaxRole>,
}

impl HighlightSegment {
    pub(in crate::tui) fn style(&self, plain: Style) -> Style {
        match self.role {
            Some(role) => Theme::syntax(role),
            None => plain,
        }
    }
}

/// Stateful highlighter for one source stream. Feed lines in order.
pub(in crate::tui) struct BlockHighlighter {
    parse: ParseState,
    stack: ScopeStack,
}

impl BlockHighlighter {
    /// Highlighter for a fence info token such as `rust`, `ts`, or `py`, or
    /// `None` when no bundled syntax matches (callers fall back to plain
    /// styling).
    pub(in crate::tui) fn for_language(token: &str) -> Option<Self> {
        Some(Self::from_syntax(
            SYNTAX_SET.find_syntax_by_token(canonical_language_token(token))?,
        ))
    }

    /// Highlighter from a file path (`src/lib.rs`, `Makefile`, …), or `None`
    /// when the path has no bundled syntax.
    ///
    /// Does not read the file; only the path/file name/extension are used so
    /// display paths work offline. Callers should strip diff chrome (`a/`,
    /// `b/`, rename arrows) before calling.
    pub(in crate::tui) fn for_path(path: &str) -> Option<Self> {
        Some(Self::from_syntax(syntax_for_path(path)?))
    }

    fn from_syntax(syntax: &SyntaxReference) -> Self {
        Self {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
        }
    }

    /// Role segments for one source line, without a trailing newline.
    pub(in crate::tui) fn highlight_line(&mut self, line: &str) -> Vec<HighlightSegment> {
        // Syntect grammars expect the newline to drive state transitions.
        let text = format!("{line}\n");
        let Ok(ops) = self.parse.parse_line(&text, &SYNTAX_SET) else {
            return vec![HighlightSegment {
                text: line.to_string(),
                role: None,
            }];
        };
        let mut segments: Vec<HighlightSegment> = Vec::new();
        let mut cursor = 0usize;
        for (offset, op) in ops {
            let offset = offset.min(line.len());
            if offset > cursor {
                push_merged(&mut segments, &line[cursor..offset], self.scope_role());
                cursor = offset;
            }
            let _ = self.stack.apply(&op);
        }
        if cursor < line.len() {
            push_merged(&mut segments, &line[cursor..], self.scope_role());
        }
        if segments.is_empty() {
            segments.push(HighlightSegment {
                text: String::new(),
                role: None,
            });
        }
        segments
    }

    /// Innermost scope that maps onto a role, if any.
    fn scope_role(&self) -> Option<SyntaxRole> {
        for scope in self.stack.as_slice().iter().rev() {
            for (selector, role) in ROLE_SELECTORS.iter() {
                if selector.is_prefix_of(*scope) {
                    return Some(*role);
                }
            }
        }
        None
    }
}

fn push_merged(segments: &mut Vec<HighlightSegment>, text: &str, role: Option<SyntaxRole>) {
    if let Some(last) = segments.last_mut() {
        if last.role == role {
            last.text.push_str(text);
            return;
        }
    }
    segments.push(HighlightSegment {
        text: text.to_string(),
        role,
    });
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

/// Resolve a bundled syntax from a display path without opening the file.
///
/// Mirrors syntect's path/extension probe: try the full file name first so
/// names like `Makefile` and `CMakeLists.txt` win, then the extension.
fn syntax_for_path(path: &str) -> Option<&'static SyntaxReference> {
    let path = path.trim();
    if path.is_empty() || path == "/dev/null" {
        return None;
    }
    let path = Path::new(path);
    let file_name = path.file_name()?.to_str()?;
    SYNTAX_SET.find_syntax_by_extension(file_name).or_else(|| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext| SYNTAX_SET.find_syntax_by_extension(ext))
    })
}

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;
