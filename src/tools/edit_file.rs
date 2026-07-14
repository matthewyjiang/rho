use crate::{
    tool::*,
    tools::{
        diff::unified_diff,
        edit_file_args::{edit_error, input_schema, Args},
    },
};
use same_file::Handle;
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    ops::Range,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub struct EditFile;

struct FileChanges {
    path: std::path::PathBuf,
    file: tokio::fs::File,
    identity: Handle,
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
        let paths = display_paths(args, ctx);
        (!paths.is_empty()).then(|| paths.join(", "))
    }

    fn display_preview_lines(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Vec<String> {
        vec!["edit_file".into()]
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
            let paths = display_paths(args, ctx);
            let diff = if paths.len() > 1 {
                super::diff::compact_diff_for_display_with_file_headers(&result.content)
            } else {
                super::diff::compact_diff_for_display(&result.content)
            };
            if let Some(diff) = diff {
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
            let mut file = tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .await
                .map_err(|error| {
                    edit_error(index, &edit.path, format!("could not open file: {error}"))
                })?;
            let identity =
                Handle::from_file(file.try_clone().await?.into_std().await).map_err(|error| {
                    edit_error(
                        index,
                        &edit.path,
                        format!("could not identify file: {error}"),
                    )
                })?;
            let file_index = match files.iter().position(|file| file.identity == identity) {
                Some(file_index) => file_index,
                None => {
                    let mut content = String::new();
                    file.read_to_string(&mut content).await.map_err(|error| {
                        edit_error(index, &edit.path, format!("could not read file: {error}"))
                    })?;
                    files.push(FileChanges {
                        path,
                        file,
                        identity,
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

        for file in &mut files {
            file.file.rewind().await?;
            let mut current = String::new();
            file.file.read_to_string(&mut current).await?;
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
        write_all_or_rollback(&mut files).await?;

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

async fn write_all_or_rollback(files: &mut [FileChanges]) -> Result<(), ToolError> {
    let writes = files
        .iter()
        .map(|file| {
            (
                OpenOptions::new().read(true).write(true).open(&file.path),
                file.updated.clone(),
            )
        })
        .collect::<Vec<_>>();
    let originals = files
        .iter()
        .map(|file| file.original.clone())
        .collect::<Vec<_>>();
    let display_paths = files
        .iter()
        .map(|file| file.display_path.clone())
        .collect::<Vec<_>>();

    tokio::task::spawn_blocking(move || {
        for index in 0..writes.len() {
            let write_result = writes[index]
                .0
                .as_ref()
                .map_err(clone_io_error)
                .and_then(|file| write_content(file, &writes[index].1));
            if let Err(write_error) = write_result {
                let mut rollback_failures = Vec::new();
                for rollback_index in (0..=index).rev() {
                    let rollback_result = writes[rollback_index]
                        .0
                        .as_ref()
                        .map_err(clone_io_error)
                        .and_then(|file| write_content(file, &originals[rollback_index]));
                    if let Err(error) = rollback_result {
                        rollback_failures
                            .push(format!("{}: {error}", display_paths[rollback_index]));
                    }
                }
                let mut message =
                    format!("failed to write {}: {write_error}", display_paths[index]);
                if rollback_failures.is_empty() {
                    message.push_str("; all writes were rolled back");
                } else {
                    message.push_str(&format!(
                        "; rollback also failed for {}",
                        rollback_failures.join(", ")
                    ));
                }
                return Err(ToolError::Message(message));
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| ToolError::Message(format!("file write task failed: {error}")))?
}

fn write_content(file: &std::fs::File, content: &str) -> std::io::Result<()> {
    let mut file = file;
    file.seek(SeekFrom::Start(0))?;
    file.set_len(0)?;
    file.write_all(content.as_bytes())?;
    file.flush()
}

fn clone_io_error(error: &std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), error.to_string())
}

fn display_paths(args: &serde_json::Value, ctx: &ToolContext) -> Vec<String> {
    let mut paths = Vec::new();
    for path in input_paths(args) {
        let path = compact_display_path(&ctx.cwd, path);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
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
