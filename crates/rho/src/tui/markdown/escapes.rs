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
    let mut current = from;
    while let Some(offset) = text[current..].find(marker) {
        let index = current + offset;
        if !is_escaped(text, index) {
            return Some(index);
        }
        // Skip only the escaped character, not the whole marker: in `\***`,
        // the last two asterisks can still open or close bold.
        current = index + text[index..].chars().next()?.len_utf8();
    }
    None
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
