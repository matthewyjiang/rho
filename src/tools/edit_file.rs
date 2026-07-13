use crate::{
    tool::*,
    tools::{
        diff::unified_diff,
        edit_file_args::{edit_error, input_schema, Args},
    },
};
use std::{ops::Range, path::PathBuf};

pub struct EditFile;

struct FileChanges {
    path: PathBuf,
    display_path: String,
    original: String,
    updated: String,
}

#[async_trait::async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Edits one or more existing UTF-8 text files by exact string replacement. All edits are validated before any file is written.".into(),
            input_schema: input_schema(),
        }
    }

    fn display_style(&self) -> ToolDisplayStyle {
        ToolDisplayStyle::file_diff()
    }

    fn display_content(&self, args: &serde_json::Value, ctx: &ToolContext) -> Option<String> {
        let paths = input_paths(args);
        match paths.as_slice() {
            [] => None,
            [path] => Some(compact_display_path(&ctx.cwd, path)),
            paths => Some(format!("{} files", paths.len())),
        }
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
        let edits = args.into_edits()?;
        let mut files = Vec::<FileChanges>::new();
        let mut replacement_count = 0;

        for (index, edit) in edits.iter().enumerate() {
            edit.validate(index)?;
            let requested_path = resolve_path(&ctx.cwd, &edit.path);
            let path = tokio::fs::canonicalize(&requested_path)
                .await
                .map_err(|error| {
                    edit_error(
                        index,
                        &edit.path,
                        format!("could not resolve file: {error}"),
                    )
                })?;
            let file_index = match files.iter().position(|file| file.path == path) {
                Some(file_index) => file_index,
                None => {
                    let content = tokio::fs::read_to_string(&path).await.map_err(|error| {
                        edit_error(index, &edit.path, format!("could not read file: {error}"))
                    })?;
                    files.push(FileChanges {
                        path,
                        display_path: compact_display_path(&ctx.cwd, &edit.path),
                        original: content.clone(),
                        updated: content,
                    });
                    files.len() - 1
                }
            };

            let file = &mut files[file_index];
            let spans = replacement_spans(&file.updated, &edit.old_string);
            edit.validate_match_count(index, spans.len())?;
            let new_string = match_file_eol(&file.updated, &edit.new_string);
            file.updated = replace_spans(&file.updated, &spans, &new_string);
            replacement_count += spans.len();
        }

        for file in &files {
            let current = tokio::fs::read_to_string(&file.path).await?;
            if current != file.original {
                return Err(ToolError::Message(format!(
                    "{} changed while edits were being validated; no files were modified",
                    file.display_path
                )));
            }
        }

        let diffs = files
            .iter()
            .map(|file| unified_diff(&file.original, &file.updated, &file.display_path, false))
            .collect::<Vec<_>>()
            .join("\n\n");
        for file in &files {
            tokio::fs::write(&file.path, &file.updated).await?;
        }

        Ok(ToolResult {
            id,
            ok: true,
            content: truncate(
                format!(
                    "edited {} file(s); applied {} edit(s), replaced {replacement_count} occurrence(s)\n\n{diffs}",
                    files.len(),
                    edits.len()
                ),
                ctx.max_output_bytes,
            ),
        })
    }
}

fn input_paths(args: &serde_json::Value) -> Vec<&str> {
    if let Some(edits) = args.get("edits").and_then(|edits| edits.as_array()) {
        let mut paths = Vec::new();
        for path in edits
            .iter()
            .filter_map(|edit| edit.get("path").and_then(|path| path.as_str()))
        {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    } else {
        args.get("path")
            .and_then(|path| path.as_str())
            .into_iter()
            .collect()
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

fn replace_spans(content: &str, spans: &[Range<usize>], new_string: &str) -> String {
    let mut output = String::with_capacity(content.len());
    let mut last = 0;
    for span in spans {
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
#[path = "edit_file_tests.rs"]
mod tests;
