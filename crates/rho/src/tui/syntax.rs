//! Syntect-backed syntax highlighting for code fences and diff bodies.
//!
//! Parses scopes only; no syntect theme is involved. Each scope region maps
//! onto a [`SyntaxRole`] (or plain). Callers resolve roles against the active
//! palette so highlighting follows fixed, custom, and terminal-sampled themes
//! and so diff add/remove colors can supply their own plain style.
//!
//! Language grammars come from [`two_face`]'s bat-derived dump (defaults plus
//! extras such as TypeScript and TOML), not syntect's smaller default set.

use std::{cell::Cell, path::Path, sync::LazyLock};

use ratatui::{style::Style, text::Span};
use regex::RegexBuilder;
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use super::theme::{SyntaxRole, Theme};

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);

/// Soft cap on language-aware lines painted in one tool-card body pass. Beyond
/// this, remaining rows keep solid row colors so huge write/edit cards stay
/// interactive.
pub(in crate::tui) const MAX_TOOL_SYNTAX_LINES: usize = 2_500;

// Per-thread counter of syntect line parses (highlight + advance). Thread-local
// so parallel unit tests do not race the measurement.
thread_local! {
    static HIGHLIGHT_LINE_CALLS: Cell<usize> = const { Cell::new(0) };
}

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
#[derive(Clone)]
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
        record_highlight_call();
        // Syntect grammars expect the newline to drive state transitions.
        let mut text = String::with_capacity(line.len() + 1);
        text.push_str(line);
        text.push('\n');
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

    /// Advance grammar state without allocating role segments.
    ///
    /// Used for the unused side of a unified diff (old stream on context) so
    /// multi-line tokens stay aligned without paying segment cost twice.
    pub(in crate::tui) fn advance_line(&mut self, line: &str) {
        record_highlight_call();
        let mut text = String::with_capacity(line.len() + 1);
        text.push_str(line);
        text.push('\n');
        let Ok(ops) = self.parse.parse_line(&text, &SYNTAX_SET) else {
            return;
        };
        for (_, op) in ops {
            let _ = self.stack.apply(&op);
        }
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

/// Match pattern and the same semantics the grep tool used when searching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::tui) struct MatchQuery {
    pub(in crate::tui) pattern: String,
    pub(in crate::tui) literal: bool,
    pub(in crate::tui) case_sensitive: bool,
}

impl MatchQuery {
    pub(in crate::tui) fn new(
        pattern: impl Into<String>,
        literal: bool,
        case_sensitive: bool,
    ) -> Self {
        Self {
            pattern: pattern.into(),
            literal,
            case_sensitive,
        }
    }
}

/// Byte ranges of pattern matches inside `text` for search-hit overlay.
///
/// Semantics mirror the grep tool: `literal` escapes the pattern before
/// compiling, and `case_sensitive` maps onto regex case-insensitivity so
/// Unicode case pairs (for example `Ä`/`ä`) match the engine that produced the
/// hits.
pub(in crate::tui) fn match_byte_ranges(text: &str, query: &MatchQuery) -> Vec<(usize, usize)> {
    let pattern = query.pattern.as_str();
    if pattern.is_empty() || text.is_empty() {
        return Vec::new();
    }
    // Same construction as GrepRequest: escape literals, then compile once.
    let source = if query.literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    let Ok(re) = RegexBuilder::new(&source)
        .case_insensitive(!query.case_sensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
    else {
        // Invalid regex would not have produced grep hits; do not invent matches.
        return Vec::new();
    };
    re.find_iter(text).map(|m| (m.start(), m.end())).collect()
}

/// Build spans from role segments, overlaying match ranges with
/// [`Theme::search_match`]. Match ranges are byte offsets into the joined
/// segment text (the original source line).
pub(in crate::tui) fn spans_from_segments_with_matches(
    segments: &[HighlightSegment],
    plain: Style,
    match_ranges: &[(usize, usize)],
) -> Vec<Span<'static>> {
    if match_ranges.is_empty() {
        return segments
            .iter()
            .map(|segment| Span::styled(segment.text.clone(), segment.style(plain)))
            .collect();
    }
    let mut spans = Vec::new();
    let mut offset = 0usize;
    for segment in segments {
        let base = segment.style(plain);
        let text = segment.text.as_str();
        let seg_start = offset;
        let seg_end = offset + text.len();
        let mut cursor = 0usize;
        for &(m_start, m_end) in match_ranges {
            if m_end <= seg_start || m_start >= seg_end {
                continue;
            }
            let local_start = m_start.saturating_sub(seg_start).max(cursor);
            let local_end = (m_end - seg_start).min(text.len());
            if local_end <= local_start {
                continue;
            }
            if local_start > cursor {
                spans.push(Span::styled(text[cursor..local_start].to_string(), base));
            }
            spans.push(Span::styled(
                text[local_start..local_end].to_string(),
                Theme::search_match(base),
            ));
            cursor = local_end;
        }
        if cursor < text.len() {
            spans.push(Span::styled(text[cursor..].to_string(), base));
        }
        offset = seg_end;
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), plain));
    }
    spans
}

/// Plain single-segment span list with optional match overlay.
pub(in crate::tui) fn spans_plain_with_matches(
    text: &str,
    plain: Style,
    match_ranges: &[(usize, usize)],
) -> Vec<Span<'static>> {
    let segments = [HighlightSegment {
        text: text.to_string(),
        role: None,
    }];
    spans_from_segments_with_matches(&segments, plain, match_ranges)
}

fn record_highlight_call() {
    HIGHLIGHT_LINE_CALLS.with(|cell| cell.set(cell.get().saturating_add(1)));
}

/// Reset and read the highlight-line counter (tests / benches only).
#[cfg(test)]
pub(in crate::tui) fn take_highlight_line_calls() -> usize {
    HIGHLIGHT_LINE_CALLS.with(|cell| cell.replace(0))
}

/// Reset the highlight-line counter (benches / tests).
#[cfg(test)]
pub(in crate::tui) fn reset_highlight_line_calls() {
    HIGHLIGHT_LINE_CALLS.with(|cell| cell.set(0));
}

/// Warm the lazy syntax dump so first-paint benches exclude load cost.
#[cfg(test)]
pub(in crate::tui) fn warm_syntax_set() {
    LazyLock::force(&SYNTAX_SET);
    LazyLock::force(&ROLE_SELECTORS);
}

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;
