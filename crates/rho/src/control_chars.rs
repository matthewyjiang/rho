//! Text from a model or a remote party made safe to write to a terminal:
//! control characters that would steer the terminal, such as an escape
//! sequence that hides the lines after it, are written as escapes instead.

/// What [`escape_controls`] does with line breaks and tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineBreaks {
    /// `\n`, `\r`, and `\t` are escaped too, for text shown on one line.
    Escape,
    /// `\n`, `\r\n`, and `\t` are written as they are, for text shown as
    /// lines; a `\r` of its own is escaped.
    Keep,
}

/// `text` with C0 and C1 control characters and Unicode format controls,
/// such as bidirectional overrides, written as `\u{..}` escapes.
pub(crate) fn escape_controls(text: &str, breaks: LineBreaks) -> String {
    let mut escaped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match (ch, breaks) {
            // A `\r\n` line ending is a line break.
            ('\r', LineBreaks::Keep) if chars.peek() == Some(&'\n') => {}
            ('\n' | '\t', LineBreaks::Keep) => escaped.push(ch),
            ('\n', LineBreaks::Escape) => escaped.push_str("\\n"),
            ('\t', LineBreaks::Escape) => escaped.push_str("\\t"),
            ('\r', _) => escaped.push_str("\\r"),
            (ch, _) if ch.is_control() || is_unicode_format_control(ch) => {
                escaped.push_str(&format!("\\u{{{:x}}}", ch as u32));
            }
            (ch, _) => escaped.push(ch),
        }
    }
    escaped
}

fn is_unicode_format_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

#[cfg(test)]
#[path = "control_chars_tests.rs"]
mod tests;
