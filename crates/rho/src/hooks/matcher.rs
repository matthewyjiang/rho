//! Declarative selection of the tool events a hook cares about.
//!
//! Matching stays deliberately small: an exact canonical tool name or one
//! trailing `*`. There is no expression language, so what a hook selects is
//! obvious from the file and cannot depend on internal or display names.

/// Why a `tools` list was rejected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolMatcherError {
    Empty,
    BlankPattern,
    UnsupportedGlob { pattern: String },
    UnknownTool { pattern: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Pattern {
    Exact(String),
    Prefix(String),
}

/// Which tools a hook applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolMatcher {
    patterns: Option<Vec<Pattern>>,
}

impl ToolMatcher {
    /// Matches every tool. This is what omitting `tools` means.
    pub fn any() -> Self {
        Self { patterns: None }
    }

    /// Builds a matcher, rejecting names outside `canonical` so a typo fails at
    /// load rather than silently never matching.
    pub fn new(patterns: Vec<String>, canonical: &[&str]) -> Result<Self, ToolMatcherError> {
        if patterns.is_empty() {
            return Err(ToolMatcherError::Empty);
        }
        let mut parsed = Vec::with_capacity(patterns.len());
        for pattern in patterns {
            parsed.push(parse_pattern(&pattern, canonical)?);
        }
        Ok(Self {
            patterns: Some(parsed),
        })
    }

    pub fn matches(&self, tool: &str) -> bool {
        let Some(patterns) = &self.patterns else {
            return true;
        };
        patterns.iter().any(|pattern| match pattern {
            Pattern::Exact(name) => name == tool,
            Pattern::Prefix(prefix) => tool.starts_with(prefix.as_str()),
        })
    }

    /// Rendering used in the spawn contract and diagnostics.
    pub fn describe(&self) -> String {
        match &self.patterns {
            None => "*".into(),
            Some(patterns) => patterns
                .iter()
                .map(|pattern| match pattern {
                    Pattern::Exact(name) => name.clone(),
                    Pattern::Prefix(prefix) => format!("{prefix}*"),
                })
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

fn parse_pattern(pattern: &str, canonical: &[&str]) -> Result<Pattern, ToolMatcherError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(ToolMatcherError::BlankPattern);
    }
    let Some(prefix) = trimmed.strip_suffix('*') else {
        if trimmed.contains('*') {
            return Err(ToolMatcherError::UnsupportedGlob {
                pattern: trimmed.to_owned(),
            });
        }
        if !canonical.contains(&trimmed) {
            return Err(ToolMatcherError::UnknownTool {
                pattern: trimmed.to_owned(),
            });
        }
        return Ok(Pattern::Exact(trimmed.to_owned()));
    };
    if prefix.contains('*') {
        return Err(ToolMatcherError::UnsupportedGlob {
            pattern: trimmed.to_owned(),
        });
    }
    if !canonical.iter().any(|name| name.starts_with(prefix)) {
        return Err(ToolMatcherError::UnknownTool {
            pattern: trimmed.to_owned(),
        });
    }
    Ok(Pattern::Prefix(prefix.to_owned()))
}

#[cfg(test)]
#[path = "matcher_tests.rs"]
mod tests;
