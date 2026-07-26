use globset::{GlobBuilder, GlobMatcher};

use crate::tool::ToolError;

/// Matches a relative, `/`-separated workspace path against a user glob.
///
/// A pattern without a separator is anchored to any directory, so `*.rs`
/// finds nested files the way `rg -g '*.rs'` does.
pub(crate) struct PathGlob(GlobMatcher);

impl PathGlob {
    pub(crate) fn compile(pattern: &str) -> Result<Self, ToolError> {
        let anchored = if pattern.contains('/') {
            pattern.to_owned()
        } else {
            format!("**/{pattern}")
        };
        let matcher = GlobBuilder::new(&anchored)
            .literal_separator(true)
            .build()
            .map_err(|error| ToolError::Message(format!("invalid glob '{pattern}': {error}")))?
            .compile_matcher();
        Ok(Self(matcher))
    }

    pub(crate) fn matches(&self, relative: &str) -> bool {
        self.0.is_match(relative)
    }
}

#[cfg(test)]
#[path = "path_glob_tests.rs"]
mod tests;
