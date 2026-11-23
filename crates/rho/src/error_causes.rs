//! An error as one line of text with the causes under it, for messages a
//! person reads: `reqwest`'s bare "error sending request" says nothing of
//! why, and "connection refused" or a timeout is further down its chain.

use std::error::Error;

/// `error` and each cause under it, joined by `": "`. A cause the text
/// already shows, as a wrapper whose `Display` includes its source does,
/// is not repeated.
pub(crate) fn with_causes(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        let next_text = next.to_string();
        if !text.contains(&next_text) {
            text.push_str(": ");
            text.push_str(&next_text);
        }
        cause = next.source();
    }
    text
}

#[cfg(test)]
#[path = "error_causes_tests.rs"]
mod tests;
