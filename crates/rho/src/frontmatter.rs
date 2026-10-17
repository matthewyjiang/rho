//! `---`-fenced YAML frontmatter splitting shared by markdown config files
//! (SKILL.md, model prompts). Callers own YAML parsing and body policy.

/// Why a document has no complete frontmatter block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrontmatterError {
    /// The first line is not a `---` fence.
    MissingOpening,
    /// No `---` fence closes the block.
    MissingClosing,
}

/// A document split at its frontmatter fences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Split<'a> {
    /// YAML between the fences, including the last YAML line's terminator.
    pub(crate) yaml: &'a str,
    /// Everything after the closing fence line, unmodified.
    pub(crate) body: &'a str,
}

/// Splits `source` when its first line is a `---` fence. Fence lines accept
/// LF or CRLF endings; slices always fall on line boundaries.
pub(crate) fn split(source: &str) -> Result<Split<'_>, FrontmatterError> {
    let is_fence = |line: &str| line.trim_end_matches(['\n', '\r']) == "---";
    let mut lines = source.split_inclusive('\n');
    let opening = lines
        .next()
        .filter(|line| is_fence(line))
        .ok_or(FrontmatterError::MissingOpening)?;
    let yaml_start = opening.len();
    let mut offset = yaml_start;
    for line in lines {
        if is_fence(line) {
            return Ok(Split {
                yaml: &source[yaml_start..offset],
                body: &source[offset + line.len()..],
            });
        }
        offset += line.len();
    }
    Err(FrontmatterError::MissingClosing)
}

#[cfg(test)]
#[path = "frontmatter_tests.rs"]
mod tests;
