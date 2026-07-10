use crate::{tool::*, tools::diff::unified_diff};
use serde::Deserialize;
use serde_json::json;
use std::ops::Range;

pub struct EditFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[async_trait::async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Edits an existing UTF-8 text file by exact string replacement.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean"}},"required":["path","old_string","new_string"]}),
        }
    }

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_diff()
    }

    fn display_content(&self, args: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
        args.get("path")
            .and_then(|path| path.as_str())
            .map(|path| compact_display_path(&ctx.cwd, path))
    }

    fn display_start_lines(&self, args: &serde_json::Value, ctx: &ToolContext) -> Vec<String> {
        vec![format!(
            "edit_file {}",
            self.display_content(args, ctx).unwrap_or_default()
        )]
    }

    fn display_lines(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
        result: &ToolResult,
    ) -> Vec<String> {
        let mut lines = vec![format!(
            "edit_file {}",
            self.display_content(args, ctx)
                .unwrap_or_else(|| result.content.clone())
        )];
        if result.ok {
            if let Some(diff) = super::diff::compact_diff_for_display(&result.content) {
                lines.push(diff);
            }
        }
        lines
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        if args.old_string.is_empty() {
            return Err(ToolError::Message("old_string must not be empty".into()));
        }
        if args.old_string == args.new_string {
            return Err(ToolError::Message(
                "old_string and new_string are identical; nothing to change".into(),
            ));
        }
        let path = resolve_path(&ctx.cwd, &args.path);
        let content = std::fs::read_to_string(&path)?;
        let spans = replacement_spans(&content, &args.old_string);
        let count = spans.len();
        if count == 0 {
            return Err(ToolError::Message("old_string not found in file".into()));
        }
        if !args.replace_all && count != 1 {
            return Err(ToolError::Message(format!(
                "old_string appeared {count} times, expected exactly once"
            )));
        }
        let new_string = match_file_eol(&content, &args.new_string);
        let new_content = replace_spans(&content, &spans, &new_string, args.replace_all);
        let diff = unified_diff(
            &content,
            &new_content,
            &compact_display_path(&ctx.cwd, &args.path),
            false,
        );
        std::fs::write(&path, new_content)?;
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(
                format!(
                    "edited {}; replaced {} occurrence(s)\n\n{diff}",
                    path.display(),
                    if args.replace_all { count } else { 1 }
                ),
                ctx.max_output_bytes,
            ),
        })
    }
}

fn replacement_spans(content: &str, old_string: &str) -> Vec<Range<usize>> {
    let (content, content_map) = normalize_newlines(content);
    let (old_string, _) = normalize_newlines(old_string);
    content
        .match_indices(&old_string)
        .map(|(start, old_string)| content_map[start]..content_map[start + old_string.len()])
        .collect()
}

fn replace_spans(
    content: &str,
    spans: &[Range<usize>],
    new_string: &str,
    replace_all: bool,
) -> String {
    let mut output = String::with_capacity(content.len());
    let mut last = 0;
    for span in spans.iter().take(if replace_all { spans.len() } else { 1 }) {
        output.push_str(&content[last..span.start]);
        output.push_str(new_string);
        last = span.end;
    }
    output.push_str(&content[last..]);
    output
}

fn match_file_eol(content: &str, new_string: &str) -> String {
    let crlf = crlf_count(content);
    let lf = bare_lf_count(content);
    let cr = bare_cr_count(content);
    let eol = if cr > crlf && cr > lf {
        "\r"
    } else if crlf > lf {
        "\r\n"
    } else {
        "\n"
    };
    normalize_newlines(new_string).0.replace('\n', eol)
}

fn normalize_newlines(value: &str) -> (String, Vec<usize>) {
    let mut normalized = String::with_capacity(value.len());
    let mut map = vec![0];
    let mut chars = value.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\r' {
            let end = if matches!(chars.peek(), Some((_, '\n'))) {
                chars.next().map_or(index + 1, |(next, _)| next + 1)
            } else {
                index + 1
            };
            normalized.push('\n');
            map.push(end);
        } else {
            normalized.push(ch);
            for offset in 1..=ch.len_utf8() {
                map.push(index + offset);
            }
        }
    }
    (normalized, map)
}

fn crlf_count(value: &str) -> usize {
    value.matches("\r\n").count()
}

fn bare_lf_count(value: &str) -> usize {
    value.bytes().filter(|byte| *byte == b'\n').count() - crlf_count(value)
}

fn bare_cr_count(value: &str) -> usize {
    let bytes = value.as_bytes();
    bytes
        .iter()
        .enumerate()
        .filter(|(index, byte)| **byte == b'\r' && bytes.get(index + 1) != Some(&b'\n'))
        .count()
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn test_context() -> (TempDir, ToolContext) {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext {
            cwd: dir.path().to_path_buf(),
            max_output_bytes: 12000,
        };
        (dir, ctx)
    }

    #[tokio::test]
    async fn replaces_unique_occurrence() {
        let (_dir, ctx) = test_context();
        std::fs::write(ctx.cwd.join("sample.txt"), "alpha beta gamma").unwrap();

        let result = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "beta", "new_string": "delta"}),
                ctx.clone(),
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(ctx.cwd.join("sample.txt")).unwrap(),
            "alpha delta gamma"
        );
        assert!(result.content.contains("--- a/sample.txt"));
        assert!(result.content.contains("+++ b/sample.txt"));
        assert!(result.content.contains("-alpha beta gamma"));
        assert!(result.content.contains("+alpha delta gamma"));
    }

    #[tokio::test]
    async fn display_lines_keep_the_first_context_line() {
        let (_dir, ctx) = test_context();
        std::fs::write(ctx.cwd.join("sample.txt"), "first\nold\nlast\n").unwrap();

        let args = json!({
            "path": "sample.txt",
            "old_string": "old",
            "new_string": "new"
        });
        let result = EditFile
            .call(args.clone(), ctx.clone(), "call_1".into())
            .await
            .unwrap();

        assert_eq!(
            EditFile.display_lines(&args, &ctx, &result),
            vec!["edit_file sample.txt", "first\n-old\n+new\nlast"]
        );
    }

    #[tokio::test]
    async fn replace_all_reports_all_replacements() {
        let (_dir, ctx) = test_context();
        std::fs::write(ctx.cwd.join("sample.txt"), "old old").unwrap();

        let result = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "old", "new_string": "new", "replace_all": true}),
                ctx,
                "call_1".into(),
            )
            .await
            .unwrap();

        assert!(result.content.contains("replaced 2 occurrence(s)"));
    }

    #[tokio::test]
    async fn rejects_identical_old_and_new_string() {
        let (_dir, ctx) = test_context();
        std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

        let err = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "alpha", "new_string": "alpha"}),
                ctx,
                "call_1".into(),
            )
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "old_string and new_string are identical; nothing to change"
        );
    }

    #[tokio::test]
    async fn reports_missing_old_string() {
        let (_dir, ctx) = test_context();
        std::fs::write(ctx.cwd.join("sample.txt"), "alpha").unwrap();

        let err = EditFile
            .call(
                json!({"path": "sample.txt", "old_string": "missing", "new_string": "x"}),
                ctx,
                "call_1".into(),
            )
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "old_string not found in file");
    }

    #[tokio::test]
    async fn edits_crlf_file_with_lf_tool_strings() {
        let (root, ctx) = test_context();
        let path = root.path().join("hello.txt");
        std::fs::write(&path, "one\r\ntwo\r\nthree\r\n").unwrap();

        let result = EditFile
            .call(
                json!({"path":"hello.txt","old_string":"one\ntwo\n","new_string":"1\n2\n"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "1\r\n2\r\nthree\r\n"
        );
    }

    #[tokio::test]
    async fn edits_lf_file_with_crlf_tool_strings() {
        let (root, ctx) = test_context();
        let path = root.path().join("hello.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();

        EditFile
            .call(
                json!({"path":"hello.txt","old_string":"one\r\ntwo\r\n","new_string":"1\r\n2\r\n"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "1\n2\nthree\n");
    }

    #[tokio::test]
    async fn edits_bare_cr_file_with_lf_tool_strings() {
        let (root, ctx) = test_context();
        let path = root.path().join("hello.txt");
        std::fs::write(&path, "one\rtwo\rthree\r").unwrap();

        EditFile
            .call(
                json!({"path":"hello.txt","old_string":"one\ntwo\n","new_string":"1\n2\n"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "1\r2\rthree\r");
    }

    #[tokio::test]
    async fn replace_all_preserves_mixed_line_endings_outside_matches() {
        let (root, ctx) = test_context();
        let path = root.path().join("hello.txt");
        std::fs::write(&path, "old\r\nkeep\nold\r\n").unwrap();

        EditFile
            .call(
                json!({"path":"hello.txt","old_string":"old\n","new_string":"new\n","replace_all":true}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "new\r\nkeep\nnew\r\n"
        );
    }
}
