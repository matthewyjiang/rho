use similar::TextDiff;

const MAX_DIFF_INPUT_BYTES: usize = 128 * 1024;
const LARGE_DIFF_MESSAGE: &str = "Diff omitted: file is too large.";
pub(super) const UNREADABLE_FILE_DIFF_MESSAGE: &str =
    "Diff omitted: existing file could not be read.";

pub(super) fn unified_diff(old: &str, new: &str, display_path: &str, created: bool) -> String {
    if old == new {
        return "No changes.".into();
    }
    if old.len().saturating_add(new.len()) > MAX_DIFF_INPUT_BYTES {
        return LARGE_DIFF_MESSAGE.into();
    }

    let old_header = if created {
        "/dev/null".to_string()
    } else {
        format!("a/{display_path}")
    };
    let new_header = format!("b/{display_path}");
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(1)
        .header(&old_header, &new_header)
        .to_string()
}

/// Converts unified diffs into a compact display while preserving file names.
pub(super) fn compact_diff_for_display_with_file_headers(diff: &str) -> Option<String> {
    compact_diff_for_display_impl(diff, true)
}

/// Converts a unified diff into the compact form rendered for file tools.
///
/// File names, hunk metadata, and diff markers for unchanged lines are omitted
/// because the tool entry already identifies the operation and file.
pub(super) fn compact_diff_for_display(diff: &str) -> Option<String> {
    compact_diff_for_display_impl(diff, false)
}

fn compact_diff_for_display_impl(diff: &str, include_file_headers: bool) -> Option<String> {
    let mut in_hunk = false;
    let mut lines = Vec::new();

    for line in diff.lines() {
        if in_hunk {
            if line.is_empty() {
                in_hunk = false;
                continue;
            }
            if line.starts_with("@@") {
                continue;
            }
            if line.starts_with('\\') {
                continue;
            }

            let Some(content) = line.get(1..) else {
                continue;
            };
            match &line[..1] {
                "+" | "-" => lines.push(line.to_string()),
                " " => lines.push(content.to_string()),
                _ => {}
            }
            continue;
        }

        if let Some(path) = line.strip_prefix("+++ b/") {
            if include_file_headers {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(path.to_string());
            }
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
        }
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacts_diff_to_one_context_line_without_metadata() {
        let diff = unified_diff(
            "before\nkeep\nold\nafter\nfar away\n",
            "before\nkeep\nnew\nafter\nfar away\n",
            "hello.txt",
            false,
        );

        assert_eq!(
            compact_diff_for_display(&diff).unwrap(),
            "keep\n-old\n+new\nafter"
        );
    }

    #[test]
    fn preserves_deleted_lines_that_begin_with_diff_header_markers() {
        let diff = unified_diff(
            "before\n-- option\nafter\n",
            "before\nafter\n",
            "cli.md",
            false,
        );

        assert_eq!(
            compact_diff_for_display(&diff).unwrap(),
            "before\n--- option\nafter"
        );
    }

    #[test]
    fn compacts_multiple_file_diffs_without_headers() {
        let diff = [
            unified_diff("old one\n", "new one\n", "one.txt", false),
            unified_diff("old two\n", "new two\n", "two.txt", false),
        ]
        .join("\n\n");

        assert_eq!(
            compact_diff_for_display(&diff).unwrap(),
            "-old one\n+new one\n-old two\n+new two"
        );
    }

    #[test]
    fn compacts_multiple_file_diffs_with_file_headers() {
        let diff = [
            unified_diff("old one\n", "new one\n", "one.txt", false),
            unified_diff("old two\n", "new two\n", "nested/two.txt", false),
        ]
        .join("\n\n");

        assert_eq!(
            compact_diff_for_display_with_file_headers(&diff).unwrap(),
            "one.txt\n-old one\n+new one\n\nnested/two.txt\n-old two\n+new two"
        );
    }

    #[test]
    fn compact_diff_omits_metadata_for_created_files() {
        let diff = unified_diff("", "hello\n", "hello.txt", true);

        assert_eq!(compact_diff_for_display(&diff).unwrap(), "+hello");
    }

    #[test]
    fn omits_diff_for_large_files() {
        let large = "x".repeat(MAX_DIFF_INPUT_BYTES);

        assert_eq!(
            unified_diff(&large, "updated", "large.txt", false),
            LARGE_DIFF_MESSAGE
        );
    }

    #[test]
    fn compact_diff_returns_none_for_empty_diff() {
        assert_eq!(compact_diff_for_display(""), None);
    }

    #[test]
    fn formats_created_file() {
        let diff = unified_diff("", "hello\n", "nested/hello.txt", true);

        assert!(diff.starts_with("--- /dev/null\n+++ b/nested/hello.txt\n"));
        assert!(diff.contains("@@ -0,0 +1 @@\n+hello\n"));
    }

    #[test]
    fn formats_changed_file_with_context() {
        let diff = unified_diff(
            "before\nhello\nold\nafter\n",
            "before\nhello\nnew\nafter\n",
            "hello.txt",
            false,
        );

        assert!(diff.starts_with("--- a/hello.txt\n+++ b/hello.txt\n"));
        assert!(diff.contains(" hello\n-old\n+new\n after\n"));
    }

    #[test]
    fn reports_unchanged_content() {
        assert_eq!(
            unified_diff("same\n", "same\n", "same.txt", false),
            "No changes."
        );
    }
}
