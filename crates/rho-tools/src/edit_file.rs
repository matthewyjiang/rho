//! Single-file string replacement tool.
//!
//! One call edits one existing UTF-8 file. By default the old string must match
//! exactly once after newline normalization. Set `replace_all` to replace every
//! occurrence.

use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
};

use serde::Deserialize;
use serde_json::json;

use crate::{diff::unified_diff, tool::*, write_file::FileMutationOutcome};

pub struct EditFile;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditFileArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: bool,
}

impl EditFileArgs {
    pub(crate) fn validate(&self) -> Result<(), ToolError> {
        validate_edit_args(&self.old_string, &self.new_string)
    }
}

#[async_trait::async_trait]
impl Tool for EditFile {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "edit_file".into(),
            description: "Edits an existing UTF-8 text file by string replacement. Matching normalizes CRLF/LF newlines while preserving the file's newline style on write. By default old_string must match exactly once; set replace_all to replace every match. Prefer this for one surgical replace. Use write_file to create or fully rewrite a file, and apply_patch for multi-hunk or multi-file edits.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path of an existing file to edit."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Text to find. Must be non-empty and must differ from new_string. CRLF and LF newlines are treated equivalently when matching."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement text. Newlines are rewritten to match the file's existing style."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "When true, replace every occurrence of old_string. When false or omitted, require exactly one match."
                    }
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
        }
    }

    async fn call(
        &self,
        args: serde_json::Value,
        ctx: ToolContext,
        id: String,
    ) -> Result<ToolResult, ToolError> {
        let args: EditFileArgs = serde_json::from_value(args)?;
        let path = resolve_path(&ctx.cwd, &args.path);
        let outcome = edit_file_content(
            &path,
            &compact_display_path(&ctx.cwd, &args.path),
            &args.old_string,
            &args.new_string,
            args.replace_all,
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

/// Apply one string replacement to an existing file under an exclusive lock.
pub(super) async fn edit_file_content(
    path: &Path,
    display_path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    validate_edit_args(old_string, new_string)?;

    let path = path.to_path_buf();
    let display_path = display_path.to_string();
    let old_string = old_string.to_string();
    let new_string = new_string.to_string();
    tokio::task::spawn_blocking(move || {
        edit_file_content_locked(
            &path,
            &display_path,
            &old_string,
            &new_string,
            replace_all,
            max_output_bytes,
        )
    })
    .await
    .map_err(|error| ToolError::Message(format!("edit task failed: {error}")))?
}

fn edit_file_content_locked(
    path: &Path,
    display_path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    max_output_bytes: usize,
) -> Result<FileMutationOutcome, ToolError> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| ToolError::Message(format!("could not open {display_path}: {error}")))?;
    // Hold an exclusive lock across plan + write so concurrent cooperators cannot
    // change the source after validation and before commit.
    file.lock()
        .map_err(|error| ToolError::Message(format!("could not lock {display_path}: {error}")))?;

    let mut original = String::new();
    file.read_to_string(&mut original)
        .map_err(|error| ToolError::Message(format!("could not read {display_path}: {error}")))?;

    let spans = replacement_spans(&original, old_string);
    validate_match_count(display_path, spans.len(), replace_all)?;
    let replacement = match_file_eol(&original, new_string);
    let updated = replace_spans(&original, &spans, &replacement);

    file.seek(SeekFrom::Start(0)).map_err(|error| {
        ToolError::Message(format!("could not rewrite {display_path}: {error}"))
    })?;
    file.set_len(0).map_err(|error| {
        ToolError::Message(format!("could not rewrite {display_path}: {error}"))
    })?;
    file.write_all(updated.as_bytes()).map_err(|error| {
        ToolError::Message(format!("could not write {display_path}: {error}"))
    })?;
    file.flush()
        .map_err(|error| ToolError::Message(format!("could not write {display_path}: {error}")))?;

    let diff = unified_diff(
        &original,
        &updated,
        display_path,
        /*created*/ false,
    );
    let replaced = spans.len();
    Ok(FileMutationOutcome {
        content: truncate(
            format!("edited {display_path}; replaced {replaced} occurrence(s)\n\n{diff}"),
            max_output_bytes,
        ),
        display_path: display_path.to_string(),
        diff,
    })
}

fn validate_edit_args(old_string: &str, new_string: &str) -> Result<(), ToolError> {
    if old_string.is_empty() {
        return Err(ToolError::Message("old_string must not be empty".into()));
    }
    if old_string == new_string {
        return Err(ToolError::Message(
            "old_string and new_string are identical; nothing to change".into(),
        ));
    }
    Ok(())
}

fn validate_match_count(
    display_path: &str,
    actual: usize,
    replace_all: bool,
) -> Result<(), ToolError> {
    if replace_all {
        if actual == 0 {
            return Err(ToolError::Message(format!(
                "edit {display_path} failed: missing match: found 0 occurrence(s)"
            )));
        }
        return Ok(());
    }
    if actual == 1 {
        return Ok(());
    }
    let reason = if actual == 0 {
        "missing match"
    } else {
        "ambiguous match"
    };
    Err(ToolError::Message(format!(
        "edit {display_path} failed: {reason}: found {actual} occurrence(s), expected 1"
    )))
}

fn replacement_spans(content: &str, old_string: &str) -> Vec<Range<usize>> {
    let (content, content_map) = normalize_newlines(content);
    let (old_string, _) = normalize_newlines(old_string);
    content
        .match_indices(&old_string)
        .map(|(start, matched)| content_map[start]..content_map[start + matched.len()])
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
