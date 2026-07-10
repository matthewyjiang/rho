use similar::TextDiff;

pub(super) fn unified_diff(old: &str, new: &str, display_path: &str, created: bool) -> String {
    if old == new {
        return "No changes.".into();
    }

    let old_header = if created {
        "/dev/null".to_string()
    } else {
        format!("a/{display_path}")
    };
    let new_header = format!("b/{display_path}");
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&old_header, &new_header)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
