use crate::tui::syntax::BlockHighlighter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct CodeFence {
    pub(super) marker: char,
    pub(super) length: usize,
}

pub(super) struct MermaidOpeningFence {
    pub(super) fence: CodeFence,
}

/// Open/closed fence tracker for streaming markdown. Carries the info-string
/// language and the syntect lexical state so live preview can keep highlighting
/// continuation lines (including multi-line strings/comments) correctly.
#[derive(Clone, Default)]
pub(in crate::tui) struct CodeFenceState {
    pub(in crate::tui) active: Option<CodeFence>,
    /// Lowercased first info-string token from the opening fence, when present.
    pub(in crate::tui) language: Option<String>,
    /// Highlighter advanced through committed body lines of the open fence.
    /// Cloned into live-preview renders; taken/restored by full renders.
    pub(in crate::tui) highlighter: Option<BlockHighlighter>,
}

impl CodeFenceState {
    pub(in crate::tui) fn is_open(&self) -> bool {
        self.active.is_some()
    }

    pub(super) fn clear_open(&mut self) {
        self.active = None;
        self.language = None;
        self.highlighter = None;
    }

    /// Record an opening fence. Leaves the highlighter on `self` so
    /// [`update_code_block_state`] can advance it; render paths that move the
    /// highlighter onto an active block should take it afterward.
    pub(super) fn open_fence(&mut self, fence: CodeFence, language: Option<String>) {
        self.highlighter = language.as_deref().and_then(BlockHighlighter::for_language);
        self.active = Some(fence);
        self.language = language;
    }
}

pub(in crate::tui) fn update_code_block_state(text: &str, state: &mut CodeFenceState) {
    for line in text.lines() {
        if state
            .active
            .is_some_and(|fence| is_closing_fence(line, fence))
        {
            state.clear_open();
        } else if state.active.is_none() {
            if let Some(fence) = parse_opening_fence(line) {
                state.open_fence(fence, opening_fence_info_token(line));
            }
        } else if let Some(highlighter) = state.highlighter.as_mut() {
            // Advance lexical state for committed body lines so a later
            // preview/render chunk resumes inside multi-line tokens.
            highlighter.advance_line(line);
        }
    }
}

pub(in crate::tui) fn parse_opening_fence(line: &str) -> Option<CodeFence> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker = rest.chars().next()?;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let length = rest
        .chars()
        .take_while(|&character| character == marker)
        .count();
    if length < 3 {
        return None;
    }
    let info = &rest[length..];
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some(CodeFence { marker, length })
}

pub(in crate::tui) fn is_closing_fence(line: &str, opening: CodeFence) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return false;
    }
    let rest = &line[indent..];
    let length = rest
        .chars()
        .take_while(|&character| character == opening.marker)
        .count();
    length >= opening.length && rest[length..].chars().all(char::is_whitespace)
}

/// Lowercased first info-string token of an opening fence line, when present.
pub(in crate::tui) fn opening_fence_info_token(line: &str) -> Option<String> {
    let fence = parse_opening_fence(line)?;
    let indent = line.len() - line.trim_start_matches(' ').len();
    let rest = &line[indent + fence.length..];
    rest.split_whitespace().next().map(str::to_ascii_lowercase)
}

pub(super) fn mermaid_opening_fence(line: &str) -> Option<MermaidOpeningFence> {
    let fence = parse_opening_fence(line)?;
    (opening_fence_info_token(line).as_deref() == Some("mermaid"))
        .then_some(MermaidOpeningFence { fence })
}
