use crate::{
    tool::*,
    tools::{
        diff::unified_diff,
        edit_file_args::{edit_error, input_schema, Args, Edit},
    },
};
use same_file::Handle;
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    ops::Range,
    path::PathBuf,
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub struct EditFile;

struct FileChanges {
    path: PathBuf,
    file: tokio::fs::File,
    identity: Handle,
    display_path: String,
    original: String,
    updated: String,
}

pub(super) struct EditFileOutcome {
    pub content: String,
    pub display_paths: Vec<String>,
    pub diffs: String,
    pub file_count: usize,
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

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: Args = serde_json::from_value(args)?;
        let edits = args.into_edits()?;
        let cwd = ctx.cwd.clone();
        let outcome = apply_edits(
            edits,
            |path| resolve_path(&cwd, path),
            |path| compact_display_path(&cwd, path),
            ctx.max_output_bytes,
        )
        .await?;
        Ok(ToolResult {
            id,
            ok: true,
            content: outcome.content,
        })
    }
}

pub(super) async fn apply_edits(
    edits: Vec<Edit>,
    resolve_requested: impl Fn(&str) -> PathBuf,
    display_path: impl Fn(&str) -> String,
    max_output_bytes: usize,
) -> Result<EditFileOutcome, ToolError> {
    let mut files = Vec::<FileChanges>::new();
    let mut replacement_count = 0;
    let edit_count = edits.len();

    for (index, edit) in edits.iter().enumerate() {
        edit.validate(index)?;
        let requested_path = resolve_requested(&edit.path);
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
                    display_path: display_path(&edit.path),
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
    let display_paths = files.iter().map(|file| file.display_path.clone()).collect();
    let file_count = files.len();
    write_all_or_rollback(&mut files).await?;

    Ok(EditFileOutcome {
        content: truncate(
            format!(
                "edited {file_count} file(s); applied {edit_count} edit(s), replaced {replacement_count} occurrence(s)\n\n{diffs}"
            ),
            max_output_bytes,
        ),
        display_paths,
        diffs,
        file_count,
    })
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
