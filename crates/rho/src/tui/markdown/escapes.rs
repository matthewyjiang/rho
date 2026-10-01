//! Markdown backslash escapes apply to ASCII punctuation, except inside code
//! and math spans, whose contents belong to their own syntax.

pub(in crate::tui) fn is_escaped(text: &str, index: usize) -> bool {
    text.as_bytes()[..index]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

pub(in crate::tui) fn find_unescaped(text: &str, marker: &str, from: usize) -> Option<usize> {
    text[from..]
        .match_indices(marker)
        .map(|(index, _)| from + index)
        .find(|index| !is_escaped(text, *index))
}

pub(in crate::tui) fn unescape(text: &str) -> String {
    let mut chars = text.chars().peekable();
    let mut output = String::with_capacity(text.len());
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
            output.push(chars.next().expect("peeked punctuation"));
        } else {
            output.push(ch);
        }
    }
    output
}
