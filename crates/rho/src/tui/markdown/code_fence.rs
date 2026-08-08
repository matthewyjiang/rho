#[derive(Clone, Copy)]
pub(in crate::tui) struct CodeFence {
    pub(super) marker: char,
    pub(super) length: usize,
}

pub(super) struct MermaidOpeningFence {
    pub(super) fence: CodeFence,
}

#[derive(Clone, Copy, Default)]
pub(in crate::tui) struct CodeFenceState {
    pub(super) active: Option<CodeFence>,
}

impl CodeFenceState {
    pub(in crate::tui) fn is_open(self) -> bool {
        self.active.is_some()
    }

    pub(super) fn from_open_flag(is_open: bool) -> Self {
        Self {
            active: is_open.then_some(CodeFence {
                marker: '`',
                length: 3,
            }),
        }
    }
}

pub(in crate::tui) fn update_code_block_state(text: &str, state: &mut CodeFenceState) {
    for line in text.lines() {
        if state
            .active
            .is_some_and(|fence| is_closing_fence(line, fence))
        {
            state.active = None;
        } else if state.active.is_none() {
            state.active = parse_opening_fence(line);
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
///
/// Fences that opened in an earlier render entry lose their info string, so
/// callers must treat `None` as "plain code".
pub(super) fn opening_fence_info_token(line: &str) -> Option<String> {
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
