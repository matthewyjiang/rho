//! Git `--patch` text for one file change, in ACP v2's `git_patch` format:
//! Git's layout, but with absolute paths and no `a/`/`b/` prefixes.

use std::path::Path;

use similar::TextDiff;

/// One file's text before and after a change.
#[derive(Clone, Copy, Debug)]
pub(super) enum FileContents<'a> {
    Created(&'a str),
    Modified { old: &'a str, new: &'a str },
    Deleted(&'a str),
}

/// The patch for the change to the file at `path`. `None` when there is
/// nothing to render or the path needs Git's quoting, which v2 makes
/// optional: the change list stays authoritative.
pub(super) fn file_patch(path: &Path, contents: FileContents<'_>) -> Option<String> {
    let path = plain_absolute(path)?;
    let (mode_line, from, to, old, new) = match contents {
        FileContents::Created(new) => ("new file mode 100644\n", "/dev/null", path, "", new),
        FileContents::Modified { old, new } => ("", path, path, old, new),
        FileContents::Deleted(old) => ("deleted file mode 100644\n", path, "/dev/null", old, ""),
    };
    let hunks = TextDiff::from_lines(old, new)
        .unified_diff()
        .header(from, to)
        .to_string();
    if hunks.is_empty() && mode_line.is_empty() {
        return None;
    }
    Some(format!("diff --git {path} {path}\n{mode_line}{hunks}"))
}

/// `path` as patch text, when Git would print it unquoted.
fn plain_absolute(path: &Path) -> Option<&str> {
    let text = path.to_str()?;
    let needs_quoting = text
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control() || !c.is_ascii());
    (path.is_absolute() && !needs_quoting).then_some(text)
}

#[cfg(test)]
#[path = "git_patch_tests.rs"]
mod tests;
