//! Locate and decode a POSIX-style shell word without executing expansions.

use super::{FileMention, PathTokenSource};

/// Keep quoted whitespace in the token and decode only the portion before the
/// cursor. Replacement still covers the whole raw token, including its quotes.
pub(in crate::tui) fn shell_word_at_cursor(input: &str, cursor: usize) -> FileMention {
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut query = String::new();
    let mut end = 0;
    let mut chars = input.chars().enumerate().peekable();
    while let Some((index, ch)) = chars.next() {
        end = index + 1;
        if !escaped && quote.is_none() && ch.is_whitespace() {
            if index >= cursor {
                end = index;
                break;
            }
            start = index + 1;
            query.clear();
            continue;
        }
        let decoded = if escaped {
            escaped = false;
            Some(ch)
        } else if ch == '\\'
            && quote != Some('\'')
            && (quote.is_none()
                || chars
                    .peek()
                    .is_some_and(|(_, next)| matches!(next, '$' | '`' | '"' | '\\' | '\n')))
        {
            escaped = true;
            None
        } else if quote == Some(ch) {
            quote = None;
            None
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
            None
        } else {
            Some(ch)
        };
        if let Some(ch) = decoded.filter(|_| index < cursor) {
            query.push(ch);
        }
    }
    FileMention {
        start,
        end,
        query,
        source: PathTokenSource::ShellWord,
    }
}

#[cfg(test)]
#[path = "shell_word_tests.rs"]
mod tests;
