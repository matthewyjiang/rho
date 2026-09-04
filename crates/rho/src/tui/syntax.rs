//! Syntect-backed syntax highlighting for code fences, diffs, and shell cards.
//!
//! Parses scopes only; no syntect theme is involved. Each scope region maps
//! onto a [`SyntaxRole`] (or plain). Callers resolve roles against the active
//! palette so highlighting follows fixed, custom, and terminal-sampled themes
//! and so diff add/remove colors can supply their own plain style.
//!
//! Language grammars come from [`two_face`]'s bat-derived dump (defaults plus
//! extras such as TypeScript and TOML), with a small bundled PowerShell grammar
//! because two-face excludes PowerShell when using the fancy-regex backend.
//! Interactive startup loads that dump off the UI thread; lookups stay `None`
//! until it is ready so the first resume frame does not hitch.

use std::{
    cell::Cell,
    path::Path,
    sync::{LazyLock, OnceLock},
};

use ratatui::{style::Style, text::Span};
use regex::{Regex, RegexBuilder};
use syntect::parsing::{
    ParseState, Scope, ScopeStack, SyntaxDefinition, SyntaxReference, SyntaxSet,
};

use super::theme::{SyntaxRole, Theme};

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static POWERSHELL_SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();

/// Ready set, or `None` until [`warm_syntax_set`] finishes.
fn syntax_set() -> Option<&'static SyntaxSet> {
    SYNTAX_SET.get()
}

fn powershell_syntax_set() -> Option<&'static SyntaxSet> {
    POWERSHELL_SYNTAX_SET.get()
}

/// Whether syntax-backed render caches may retain their current paint.
pub(in crate::tui) fn syntax_set_ready() -> bool {
    syntax_set().is_some() && powershell_syntax_set().is_some()
}

/// Inflate the bat dump and role selectors. Safe to call more than once.
pub(crate) fn warm_syntax_set() {
    let _ = SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines);
    let _ = POWERSHELL_SYNTAX_SET.get_or_init(load_powershell_syntax_set);
    LazyLock::force(&ROLE_SELECTORS);
}

fn load_powershell_syntax_set() -> SyntaxSet {
    let mut builder = SyntaxSet::new().into_builder();
    let powershell = SyntaxDefinition::load_from_str(
        include_str!("powershell.sublime-syntax"),
        /*lines_include_newline*/ true,
        None,
    )
    .expect("bundled PowerShell syntax must be valid");
    builder.add(powershell);
    builder.build()
}

/// Dump-native syntax name for a fence token, when the set is ready and known.
pub(in crate::tui) fn syntax_name_for_language(token: &str) -> Option<&'static str> {
    Some(syntax_for_language(token)?.name.as_str())
}

/// Dump-native syntax name for a display path, when the set is ready and known.
pub(in crate::tui) fn syntax_name_for_path(path: &str) -> Option<&'static str> {
    Some(syntax_for_path(path)?.name.as_str())
}

/// Soft cap on language-aware lines painted in one tool-card source pass.
/// Beyond this, content stays plain so huge cards remain interactive.
pub(in crate::tui) const MAX_TOOL_SYNTAX_LINES: usize = 2_500;

/// Soft cap on bytes per line for language-aware tool-card paint. Longer rows
/// keep their caller-provided plain style.
///
/// Syntect's Markdown grammar is pathological on dense inline-code spans: a
/// single ~800-byte docs prose line with many `` `backticks` `` can take tens
/// of milliseconds. Expand paints both diff sides on the UI thread, so one
/// long `.md` edit felt like a stall. Line-count caps alone do not catch this.
pub(in crate::tui) const MAX_TOOL_SYNTAX_LINE_BYTES: usize = 256;

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
            // Patch so a diff row wash (or other caller bg) survives the role fg.
            Some(role) => plain.patch(Theme::syntax(role)),
            None => plain,
        }
    }
}

/// Stateful highlighter for one source stream. Feed lines in order.
#[derive(Clone)]
pub(in crate::tui) struct BlockHighlighter {
    parse: ParseState,
    stack: ScopeStack,
    syntax_set: &'static SyntaxSet,
}

impl BlockHighlighter {
    /// Highlighter for a fence info token such as `rust`, `ts`, or `py`, or
    /// `None` when no bundled syntax matches (callers fall back to plain
    /// styling).
    pub(in crate::tui) fn for_language(token: &str) -> Option<Self> {
        let token = canonical_language_token(token);
        let set = syntax_set_for_language(token)?;
        Some(Self::from_syntax(set.find_syntax_by_token(token)?, set))
    }

    /// Highlighter from a file path (`src/lib.rs`, `Makefile`, …), or `None`
    /// when the path has no bundled syntax.
    ///
    /// Does not read the file; only the path/file name/extension are used so
    /// display paths work offline. Callers should strip diff chrome (`a/`,
    /// `b/`, rename arrows) before calling.
    pub(in crate::tui) fn for_path(path: &str) -> Option<Self> {
        let (syntax, set) = syntax_for_path_with_set(path)?;
        Some(Self::from_syntax(syntax, set))
    }

    fn from_syntax(syntax: &SyntaxReference, syntax_set: &'static SyntaxSet) -> Self {
        Self {
            parse: ParseState::new(syntax),
            stack: ScopeStack::new(),
            syntax_set,
        }
    }

    /// Role segments for one source line, without a trailing newline.
    pub(in crate::tui) fn highlight_line(&mut self, line: &str) -> Vec<HighlightSegment> {
        record_highlight_call();
        // Syntect grammars expect the newline to drive state transitions.
        let mut text = String::with_capacity(line.len() + 1);
        text.push_str(line);
        text.push('\n');
        let Ok(ops) = self.parse.parse_line(&text, self.syntax_set) else {
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
        let Ok(ops) = self.parse.parse_line(&text, self.syntax_set) else {
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
        "ps1" => "powershell",
        "shell" | "console" => "bash",
        other => other,
    }
}

fn syntax_set_for_language(token: &str) -> Option<&'static SyntaxSet> {
    if token == "powershell" {
        powershell_syntax_set()
    } else {
        syntax_set()
    }
}

fn syntax_for_language(token: &str) -> Option<&'static SyntaxReference> {
    let token = canonical_language_token(token);
    syntax_set_for_language(token)?.find_syntax_by_token(token)
}

/// Resolve a bundled syntax from a display path without opening the file.
///
/// Mirrors syntect's path/extension probe: try the full file name first so
/// names like `Makefile` and `CMakeLists.txt` win, then the extension.
fn syntax_for_path(path: &str) -> Option<&'static SyntaxReference> {
    Some(syntax_for_path_with_set(path)?.0)
}

fn syntax_for_path_with_set(path: &str) -> Option<(&'static SyntaxReference, &'static SyntaxSet)> {
    let path = path.trim();
    if path.is_empty() || path == "/dev/null" {
        return None;
    }
    let path = Path::new(path);
    let file_name = path.file_name()?.to_str()?;
    let extension = path.extension().and_then(|ext| ext.to_str());
    let set = if extension.is_some_and(|ext| matches!(ext, "ps1" | "psm1" | "psd1")) {
        powershell_syntax_set()?
    } else {
        syntax_set()?
    };
    let syntax = set.find_syntax_by_extension(file_name).or_else(|| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext| set.find_syntax_by_extension(ext))
    })?;
    Some((syntax, set))
}

/// Match pattern and the same semantics the grep tool used when searching.
///
/// The overlay regex is compiled once in [`MatchQuery::new`] so search-body
/// paint can reuse it on every line instead of rebuilding it per row.
#[derive(Clone, Debug)]
pub(in crate::tui) struct MatchQuery {
    pattern: String,
    literal: bool,
    case_sensitive: bool,
    compiled: Option<Regex>,
}

impl PartialEq for MatchQuery {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
            && self.literal == other.literal
            && self.case_sensitive == other.case_sensitive
    }
}

impl Eq for MatchQuery {}

impl MatchQuery {
    pub(in crate::tui) fn new(
        pattern: impl Into<String>,
        literal: bool,
        case_sensitive: bool,
    ) -> Self {
        let pattern = pattern.into();
        let compiled = compile_match_regex(&pattern, literal, case_sensitive);
        Self {
            pattern,
            literal,
            case_sensitive,
            compiled,
        }
    }
}

/// Same construction as `GrepRequest`: escape literals, then compile once.
fn compile_match_regex(pattern: &str, literal: bool, case_sensitive: bool) -> Option<Regex> {
    if pattern.is_empty() {
        return None;
    }
    let source = if literal {
        regex::escape(pattern)
    } else {
        pattern.to_string()
    };
    RegexBuilder::new(&source)
        .case_insensitive(!case_sensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .ok()
}

/// Byte ranges of pattern matches inside `text` for search-hit overlay.
///
/// Semantics mirror the grep tool: `literal` escapes the pattern before
/// compiling, and `case_sensitive` maps onto regex case-insensitivity so
/// Unicode case pairs (for example `Ä`/`ä`) match the engine that produced the
/// hits.
pub(in crate::tui) fn match_byte_ranges(text: &str, query: &MatchQuery) -> Vec<(usize, usize)> {
    let Some(re) = query.compiled.as_ref() else {
        return Vec::new();
    };
    if text.is_empty() {
        return Vec::new();
    }
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

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;
