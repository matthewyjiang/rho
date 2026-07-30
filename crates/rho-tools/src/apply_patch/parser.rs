//! Patch grammar types and markers compatible with OpenAI Codex `apply_patch`.
//!
//! Format adapted from the Apache-2.0 codex-rs apply-patch crate.

use std::fmt;
use std::path::{Path, PathBuf};

pub(super) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub(super) const END_PATCH_MARKER: &str = "*** End Patch";
pub(super) const ADD_FILE_MARKER: &str = "*** Add File: ";
pub(super) const DELETE_FILE_MARKER: &str = "*** Delete File: ";
pub(super) const UPDATE_FILE_MARKER: &str = "*** Update File: ";
pub(super) const MOVE_TO_MARKER: &str = "*** Move to: ";
pub(super) const EOF_MARKER: &str = "*** End of File";
pub(super) const CHANGE_CONTEXT_MARKER: &str = "@@ ";
pub(super) const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";
pub(super) const ENVIRONMENT_ID_MARKER: &str = "*** Environment ID:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hunk {
    Add {
        path: PathBuf,
        contents: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        move_path: Option<PathBuf>,
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    pub(crate) fn source_path(&self) -> &Path {
        match self {
            Self::Add { path, .. } | Self::Delete { path } | Self::Update { path, .. } => path,
        }
    }

    pub(crate) fn affected_paths(&self) -> Vec<&Path> {
        match self {
            Self::Add { path, .. } | Self::Delete { path } => vec![path.as_path()],
            Self::Update {
                path,
                move_path: Some(dest),
                ..
            } => vec![path.as_path(), dest.as_path()],
            Self::Update {
                path,
                move_path: None,
                ..
            } => vec![path.as_path()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateFileChunk {
    pub(crate) change_context: Option<String>,
    pub(crate) old_lines: Vec<String>,
    pub(crate) new_lines: Vec<String>,
    pub(crate) is_end_of_file: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    InvalidPatch(String),
    InvalidHunk { message: String, line_number: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPatch(message) => write!(f, "invalid patch: {message}"),
            Self::InvalidHunk {
                message,
                line_number,
            } => write!(f, "invalid hunk at line {line_number}, {message}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse a Codex-style apply_patch document into file operations.
pub(crate) fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    let patch = strip_lenient_heredoc(patch.trim());
    let mut parser = StreamingPatchParser::default();
    parser.push_delta(&patch)?;
    let hunks = parser.finish()?;
    if hunks.is_empty() {
        return Err(ParseError::InvalidPatch(
            "patch must contain at least one file operation".into(),
        ));
    }
    Ok(hunks)
}

fn strip_lenient_heredoc(patch: &str) -> String {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.len() >= 2 {
        let first = lines[0].trim();
        let last = lines[lines.len() - 1].trim();
        let heredoc_start = (first.starts_with("<<'") && first.ends_with('\''))
            || (first.starts_with("<<\"") && first.ends_with('"'))
            || first.starts_with("<<");
        if heredoc_start && (last == "EOF" || last == "PATCH" || first.contains(last)) {
            return lines[1..lines.len() - 1].join("\n");
        }
    }
    patch.to_string()
}

#[derive(Debug, Default)]
struct StreamingPatchParser {
    line_buffer: String,
    mode: Mode,
    hunks: Vec<Hunk>,
    line_number: usize,
    environment_seen: bool,
}

#[derive(Debug, Default, Clone, Copy)]
enum Mode {
    #[default]
    NotStarted,
    StartedPatch,
    AddFile,
    DeleteFile,
    UpdateFile {
        hunk_line_number: usize,
    },
    EndedPatch,
}

impl StreamingPatchParser {
    fn push_delta(&mut self, delta: &str) -> Result<(), ParseError> {
        for ch in delta.chars() {
            if ch == '\n' {
                let mut line = std::mem::take(&mut self.line_buffer);
                if let Some(stripped) = line.strip_suffix('\r') {
                    line.truncate(stripped.len());
                }
                self.line_number += 1;
                self.process_line(&line)?;
            } else {
                self.line_buffer.push(ch);
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Vec<Hunk>, ParseError> {
        if !self.line_buffer.is_empty() {
            let line = std::mem::take(&mut self.line_buffer);
            self.line_number += 1;
            if line.trim() == END_PATCH_MARKER {
                self.ensure_update_hunk_is_not_empty(line.trim())?;
                self.mode = Mode::EndedPatch;
            } else {
                self.process_line(&line)?;
            }
        }
        if !matches!(self.mode, Mode::EndedPatch) {
            return Err(ParseError::InvalidPatch(
                "The last line of the patch must be '*** End Patch'".into(),
            ));
        }
        Ok(std::mem::take(&mut self.hunks))
    }

    fn ensure_update_hunk_is_not_empty(&self, line: &str) -> Result<(), ParseError> {
        let Some(Hunk::Update { path, chunks, .. }) = self.hunks.last() else {
            return Ok(());
        };
        if chunks.is_empty() {
            if let Mode::UpdateFile { hunk_line_number } = self.mode {
                return Err(ParseError::InvalidHunk {
                    message: format!("Update file hunk for path '{}' is empty", path.display()),
                    line_number: hunk_line_number,
                });
            }
        }
        if chunks
            .last()
            .is_some_and(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty())
        {
            if line == END_PATCH_MARKER {
                return Err(ParseError::InvalidHunk {
                    message: "Update hunk does not contain any lines".into(),
                    line_number: self.line_number,
                });
            }
            return Err(ParseError::InvalidHunk {
                message: format!(
                    "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                ),
                line_number: self.line_number,
            });
        }
        Ok(())
    }

    fn handle_headers(&mut self, trimmed: &str) -> Result<bool, ParseError> {
        if matches!(self.mode, Mode::StartedPatch) {
            if let Some(environment_id) = trimmed.strip_prefix(ENVIRONMENT_ID_MARKER) {
                if self.environment_seen {
                    return Err(ParseError::InvalidPatch(
                        "apply_patch environment_id cannot be specified more than once".into(),
                    ));
                }
                if environment_id.trim().is_empty() {
                    return Err(ParseError::InvalidPatch(
                        "apply_patch environment_id cannot be empty".into(),
                    ));
                }
                self.environment_seen = true;
                return Ok(true);
            }
        }
        if trimmed == END_PATCH_MARKER {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.mode = Mode::EndedPatch;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(Hunk::Add {
                path: PathBuf::from(path),
                contents: String::new(),
            });
            self.mode = Mode::AddFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(Hunk::Delete {
                path: PathBuf::from(path),
            });
            self.mode = Mode::DeleteFile;
            return Ok(true);
        }
        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            self.ensure_update_hunk_is_not_empty(trimmed)?;
            self.hunks.push(Hunk::Update {
                path: PathBuf::from(path),
                move_path: None,
                chunks: Vec::new(),
            });
            self.mode = Mode::UpdateFile {
                hunk_line_number: self.line_number,
            };
            return Ok(true);
        }
        Ok(false)
    }

    fn invalid_header(trimmed: &str, line_number: usize) -> ParseError {
        ParseError::InvalidHunk {
            message: format!(
                "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
            ),
            line_number,
        }
    }

    fn process_line(&mut self, line: &str) -> Result<(), ParseError> {
        let trimmed = line.trim();
        match self.mode {
            Mode::NotStarted => {
                if trimmed == BEGIN_PATCH_MARKER {
                    self.mode = Mode::StartedPatch;
                    Ok(())
                } else {
                    Err(ParseError::InvalidPatch(
                        "The first line of the patch must be '*** Begin Patch'".into(),
                    ))
                }
            }
            Mode::StartedPatch => {
                if self.handle_headers(trimmed)? {
                    Ok(())
                } else {
                    Err(Self::invalid_header(trimmed, self.line_number))
                }
            }
            Mode::AddFile => {
                if self.handle_headers(trimmed)? {
                    return Ok(());
                }
                if let Some(added) = line.strip_prefix('+') {
                    if let Some(Hunk::Add { contents, .. }) = self.hunks.last_mut() {
                        contents.push_str(added);
                        contents.push('\n');
                        return Ok(());
                    }
                }
                Err(Self::invalid_header(trimmed, self.line_number))
            }
            Mode::DeleteFile => {
                if self.handle_headers(trimmed)? {
                    Ok(())
                } else {
                    Err(Self::invalid_header(trimmed, self.line_number))
                }
            }
            Mode::UpdateFile { hunk_line_number } => {
                self.process_update_line(line, hunk_line_number)
            }
            Mode::EndedPatch => {
                if trimmed.is_empty() {
                    Ok(())
                } else {
                    Err(ParseError::InvalidPatch(
                        "The last line of the patch must be '*** End Patch'".into(),
                    ))
                }
            }
        }
    }

    fn process_update_line(
        &mut self,
        line: &str,
        hunk_line_number: usize,
    ) -> Result<(), ParseError> {
        let update_line = line.trim_end();
        if self.handle_headers(update_line)? {
            return Ok(());
        }

        let Some(Hunk::Update {
            move_path, chunks, ..
        }) = self.hunks.last_mut()
        else {
            return Ok(());
        };

        if chunks.last().is_some_and(|chunk| chunk.is_end_of_file) {
            if update_line.is_empty() {
                return Ok(());
            }
            if update_line != EMPTY_CHANGE_CONTEXT_MARKER
                && !update_line.starts_with(CHANGE_CONTEXT_MARKER)
            {
                return Err(ParseError::InvalidHunk {
                    message: format!(
                        "Expected update hunk to start with a @@ context marker, got: '{line}'"
                    ),
                    line_number: self.line_number,
                });
            }
        }

        if chunks.is_empty() && move_path.is_none() {
            if let Some(dest) = update_line.strip_prefix(MOVE_TO_MARKER) {
                *move_path = Some(PathBuf::from(dest));
                self.mode = Mode::UpdateFile { hunk_line_number };
                return Ok(());
            }
        }

        if (update_line == EMPTY_CHANGE_CONTEXT_MARKER
            || update_line.starts_with(CHANGE_CONTEXT_MARKER))
            && chunks
                .last()
                .is_some_and(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty())
        {
            return Err(ParseError::InvalidHunk {
                message: format!(
                    "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                ),
                line_number: self.line_number,
            });
        }

        if update_line == EMPTY_CHANGE_CONTEXT_MARKER {
            chunks.push(UpdateFileChunk {
                change_context: None,
                old_lines: Vec::new(),
                new_lines: Vec::new(),
                is_end_of_file: false,
            });
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }

        if let Some(change_context) = update_line.strip_prefix(CHANGE_CONTEXT_MARKER) {
            chunks.push(UpdateFileChunk {
                change_context: Some(change_context.to_string()),
                old_lines: Vec::new(),
                new_lines: Vec::new(),
                is_end_of_file: false,
            });
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }

        if update_line == EOF_MARKER {
            if chunks.is_empty()
                || chunks
                    .last()
                    .is_some_and(|chunk| chunk.old_lines.is_empty() && chunk.new_lines.is_empty())
            {
                return Err(ParseError::InvalidHunk {
                    message: "Update hunk does not contain any lines".into(),
                    line_number: self.line_number,
                });
            }
            if let Some(chunk) = chunks.last_mut() {
                chunk.is_end_of_file = true;
            }
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }

        if line.is_empty() {
            if chunks.is_empty() {
                chunks.push(UpdateFileChunk {
                    change_context: None,
                    old_lines: Vec::new(),
                    new_lines: Vec::new(),
                    is_end_of_file: false,
                });
            }
            if let Some(chunk) = chunks.last_mut() {
                chunk.old_lines.push(String::new());
                chunk.new_lines.push(String::new());
            }
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }

        if let Some(content) = line.strip_prefix(' ') {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.old_lines.push(content.to_string());
                chunk.new_lines.push(content.to_string());
            }
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }
        if let Some(content) = line.strip_prefix('+') {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.new_lines.push(content.to_string());
            }
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }
        if let Some(content) = line.strip_prefix('-') {
            ensure_chunk(chunks);
            if let Some(chunk) = chunks.last_mut() {
                chunk.old_lines.push(content.to_string());
            }
            self.mode = Mode::UpdateFile { hunk_line_number };
            return Ok(());
        }

        if chunks
            .last()
            .is_some_and(|chunk| !chunk.old_lines.is_empty() || !chunk.new_lines.is_empty())
        {
            return Err(ParseError::InvalidHunk {
                message: format!(
                    "Expected update hunk to start with a @@ context marker, got: '{line}'"
                ),
                line_number: self.line_number,
            });
        }

        Err(ParseError::InvalidHunk {
            message: format!(
                "Unexpected line found in update hunk: '{line}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
            ),
            line_number: self.line_number,
        })
    }
}

fn ensure_chunk(chunks: &mut Vec<UpdateFileChunk>) {
    if chunks.is_empty() {
        chunks.push(UpdateFileChunk {
            change_context: None,
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            is_end_of_file: false,
        });
    }
}
