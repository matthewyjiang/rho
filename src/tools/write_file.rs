use crate::tool::*;
use serde::Deserialize;
use serde_json::json;

pub struct WriteFile;
#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

#[async_trait::async_trait]
impl Tool for WriteFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write_file".into(),
            description: "Writes a UTF-8 text file, creating or overwriting it.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        }
    }

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_or_command()
    }

    fn display_content(&self, args: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
        args.get("path")
            .and_then(|path| path.as_str())
            .map(|path| compact_display_path(&ctx.cwd, path))
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let old_content = match std::fs::read_to_string(&path) {
            Ok(content) => Some(content),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let diff = unified_diff(
            old_content.as_deref().unwrap_or(""),
            &args.content,
            &args.path,
            old_content.is_none(),
        );

        std::fs::write(&path, args.content)?;

        let action = if old_content.is_some() {
            "wrote"
        } else {
            "created"
        };
        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(
                format!("{action} {}\n\n{diff}", path.display()),
                ctx.max_output_bytes,
            ),
        })
    }
}

fn unified_diff(old: &str, new: &str, display_path: &str, created: bool) -> String {
    if old == new {
        return "No changes.".into();
    }

    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let diff_lines = diff_lines(&old_lines, &new_lines);

    let old_header = if created {
        "/dev/null".into()
    } else {
        format!("a/{display_path}")
    };
    let new_header = format!("b/{display_path}");

    let mut out = String::new();
    out.push_str(&format!("--- {old_header}\n"));
    out.push_str(&format!("+++ {new_header}\n"));
    out.push_str(&format!(
        "@@ -1,{} +1,{} @@\n",
        old_lines.len(),
        new_lines.len()
    ));

    for line in diff_lines {
        match line {
            DiffLine::Unchanged(line) => {
                out.push(' ');
                out.push_str(line);
            }
            DiffLine::Added(line) => {
                out.push('+');
                out.push_str(line);
            }
            DiffLine::Removed(line) => {
                out.push('-');
                out.push_str(line);
            }
        }
        out.push('\n');
    }

    out
}

#[derive(Debug, PartialEq, Eq)]
enum DiffLine<'a> {
    Unchanged(&'a str),
    Added(&'a str),
    Removed(&'a str),
}

fn split_lines(content: &str) -> Vec<&str> {
    content.lines().collect()
}

fn diff_lines<'a>(old: &'a [&'a str], new: &'a [&'a str]) -> Vec<DiffLine<'a>> {
    let mut lengths = vec![vec![0; new.len() + 1]; old.len() + 1];

    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i][j] = if old[i] == new[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }

    let mut lines = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        if old[i] == new[j] {
            lines.push(DiffLine::Unchanged(old[i]));
            i += 1;
            j += 1;
        } else if lengths[i + 1][j] >= lengths[i][j + 1] {
            lines.push(DiffLine::Removed(old[i]));
            i += 1;
        } else {
            lines.push(DiffLine::Added(new[j]));
            j += 1;
        }
    }

    while i < old.len() {
        lines.push(DiffLine::Removed(old[i]));
        i += 1;
    }
    while j < new.len() {
        lines.push(DiffLine::Added(new[j]));
        j += 1;
    }

    lines
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
    async fn writes_file_and_creates_parent_dirs() {
        let (root, ctx) = test_context();
        let result = WriteFile
            .call(
                json!({"path":"nested/hello.txt","content":"hello"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert_eq!(
            std::fs::read_to_string(root.path().join("nested/hello.txt")).unwrap(),
            "hello"
        );
        assert!(result.content.contains("created "));
        assert!(result.content.contains("--- /dev/null"));
        assert!(result.content.contains("+++ b/nested/hello.txt"));
        assert!(result.content.contains("+hello"));
    }

    #[tokio::test]
    async fn output_includes_diff_for_overwritten_file() {
        let (root, ctx) = test_context();
        std::fs::write(root.path().join("hello.txt"), "hello\nold\n").unwrap();

        let result = WriteFile
            .call(
                json!({"path":"hello.txt","content":"hello\nnew\n"}),
                ctx,
                "test".into(),
            )
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.content.contains("wrote "));
        assert!(result.content.contains("--- a/hello.txt"));
        assert!(result.content.contains("+++ b/hello.txt"));
        assert!(result.content.contains(" hello"));
        assert!(result.content.contains("-old"));
        assert!(result.content.contains("+new"));
    }

    #[test]
    fn formats_no_changes() {
        assert_eq!(
            unified_diff("same\n", "same\n", "same.txt", false),
            "No changes."
        );
    }
}
