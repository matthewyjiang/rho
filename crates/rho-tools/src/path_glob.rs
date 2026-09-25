use globset::{GlobBuilder, GlobMatcher};

use crate::tool::ToolError;

/// Matches a relative, `/`-separated workspace path against a user glob.
///
/// A pattern without a separator is anchored to any directory, so `*.rs`
/// finds nested files the way `rg -g '*.rs'` does. A leading `!` inverts the
/// match, so `!*_tests.rs` keeps every path except test files, also like `rg -g`.
pub(crate) struct PathGlob {
    matcher: GlobMatcher,
    polarity: Polarity,
}

/// Whether a path must match (`Include`) or must not match (`Exclude`) the glob.
#[derive(Clone, Copy)]
enum Polarity {
    Include,
    Exclude,
}

impl PathGlob {
    pub(crate) fn compile(pattern: &str) -> Result<Self, ToolError> {
        let (polarity, positive) = match pattern.strip_prefix('!') {
            Some(rest) => (Polarity::Exclude, rest),
            None => (Polarity::Include, pattern),
        };
        if positive.is_empty() {
            return Err(ToolError::Message(format!(
                "invalid glob '{pattern}': empty pattern"
            )));
        }
        let anchored = if positive.contains('/') {
            positive.to_owned()
        } else {
            format!("**/{positive}")
        };
        let matcher = GlobBuilder::new(&anchored)
            .literal_separator(true)
            .build()
            .map_err(|error| ToolError::Message(format!("invalid glob '{pattern}': {error}")))?
            .compile_matcher();
        Ok(Self { matcher, polarity })
    }

    pub(crate) fn matches(&self, relative: &str) -> bool {
        let hit = self.matcher.is_match(relative);
        match self.polarity {
            Polarity::Include => hit,
            Polarity::Exclude => !hit,
        }
    }
}

#[cfg(test)]
#[path = "path_glob_tests.rs"]
mod tests;
