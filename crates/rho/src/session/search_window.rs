//! The part of a matched message a search shows: a window around its first
//! match, with where each matched word falls in it.

use std::ops::Range;

/// FTS5 `highlight()` markers around each matched token, as the index's
/// excerpt queries ask for them (`char(30)`, `char(31)`).
pub(super) const OPEN: char = '\u{1e}';
pub(super) const CLOSE: char = '\u{1f}';

/// How much of a message a hit shows, in characters, and how many of them
/// lead its first match. The corpus median message is 575 characters, so a
/// window is about half of one.
const WINDOW_CHARS: usize = 320;
const LEAD_CHARS: usize = 80;

/// A window of a message around its first match.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Window {
    /// Its first and past-last characters in the message.
    pub start: usize,
    pub end: usize,
    pub total_chars: usize,
    pub text: String,
    /// The matched words in `text`, as byte ranges on char boundaries, in
    /// order.
    pub highlights: Vec<Range<usize>>,
}

/// The window of `text` around its first match, which `highlighted`, the
/// same text with [`OPEN`] and [`CLOSE`] around each match, marks.
///
/// Markers are found by walking both strings: a character `highlighted`
/// has where `text` does not is a marker. A marker character that `text`
/// itself holds at that place reads as text first, which at worst puts a
/// match one character off; it never splits a character.
pub(super) fn window(text: &str, highlighted: &str) -> Window {
    let mut matches = Vec::new();
    let mut source = text.char_indices().peekable();
    let mut at = 0;
    let mut open = None;
    for marked in highlighted.chars() {
        if let Some(&(index, char)) = source.peek() {
            if char == marked {
                source.next();
                at = index + char.len_utf8();
                continue;
            }
        }
        match marked {
            OPEN => open = Some(at),
            CLOSE => {
                if let Some(start) = open.take() {
                    matches.push(start..at);
                }
            }
            // `highlighted` is not `text` marked; show the start unmarked.
            _ => {
                matches.clear();
                break;
            }
        }
    }
    let first = matches
        .first()
        .map_or(0, |first| text[..first.start].chars().count());
    let start = first.saturating_sub(LEAD_CHARS);
    let byte_at = |chars: usize| {
        text.char_indices()
            .nth(chars)
            .map_or(text.len(), |(index, _)| index)
    };
    let (from, to) = (byte_at(start), byte_at(start + WINDOW_CHARS));
    let shown = &text[from..to];
    let highlights = matches
        .into_iter()
        .filter_map(|found| {
            let (begin, end) = (found.start.max(from), found.end.min(to));
            (begin < end).then(|| begin - from..end - from)
        })
        .collect();
    Window {
        start,
        end: start + shown.chars().count(),
        total_chars: text.chars().count(),
        text: shown.to_owned(),
        highlights,
    }
}

#[cfg(test)]
#[path = "search_window_tests.rs"]
mod tests;
