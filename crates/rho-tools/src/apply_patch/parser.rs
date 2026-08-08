//! Patch grammar types and line-oriented parser for Codex `apply_patch`.
//!
//! Format adapted from the Apache-2.0 codex-rs apply-patch crate. Rho always
//! receives the full patch string, so parsing is a straight line scan rather
//! than a streaming state machine.

use std::fmt;

pub(super) const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
pub(super) const END_PATCH_MARKER: &str = "*** End Patch";
pub(super) const ADD_FILE_MARKER: &str = "*** Add File: ";
pub(super) const DELETE_FILE_MARKER: &str = "*** Delete File: ";
pub(super) const UPDATE_FILE_MARKER: &str = "*** Update File: ";
pub(super) const MOVE_TO_MARKER: &str = "*** Move to: ";
pub(super) const EOF_MARKER: &str = "*** End of File";
pub(super) const CHANGE_CONTEXT_MARKER: &str = "@@ ";
pub(super) const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hunk {
    Add {
        path: String,
        contents: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_path: Option<String>,
        chunks: Vec<UpdateFileChunk>,
    },
}

impl Hunk {
    pub fn source_path(&self) -> &str {
        match self {
            Self::Add { path, .. } | Self::Delete { path } | Self::Update { path, .. } => path,
        }
    }

    pub(crate) fn requires_existing_source(&self) -> bool {
        !matches!(self, Self::Add { .. })
    }

    pub(crate) fn move_destination(&self) -> Option<&str> {
        match self {
            Self::Update {
                move_path: Some(path),
                ..
            } => Some(path),
            Self::Add { .. }
            | Self::Delete { .. }
            | Self::Update {
                move_path: None, ..
            } => None,
        }
    }

    pub(crate) fn mutates_source_entry(&self) -> bool {
        matches!(
            self,
            Self::Delete { .. }
                | Self::Update {
                    move_path: Some(_),
                    ..
                }
        )
    }

    pub fn affected_paths(&self) -> Vec<&str> {
        match self {
            Self::Add { path, .. } | Self::Delete { path } => vec![path.as_str()],
            Self::Update {
                path,
                move_path: Some(dest),
                ..
            } => vec![path.as_str(), dest.as_str()],
            Self::Update {
                path,
                move_path: None,
                ..
            } => vec![path.as_str()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFileChunk {
    pub change_context: Option<String>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub is_end_of_file: bool,
}

impl UpdateFileChunk {
    fn is_empty(&self) -> bool {
        self.old_lines.is_empty() && self.new_lines.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
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

/// Parse a complete Codex-style apply_patch document into file operations.
pub fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    let lines = patch_lines(patch.trim());
    if lines
        .first()
        .is_none_or(|line| line.trim() != BEGIN_PATCH_MARKER)
    {
        return Err(ParseError::InvalidPatch(
            "The first line of the patch must be '*** Begin Patch'".into(),
        ));
    }

    let mut index = 1usize;
    let mut hunks = Vec::new();
    let mut seen_end = false;
    while index < lines.len() {
        let line_number = index + 1;
        let trimmed = lines[index].trim();
        if trimmed == END_PATCH_MARKER {
            seen_end = true;
            index += 1;
            break;
        }
        if let Some(path) = trimmed.strip_prefix(ADD_FILE_MARKER) {
            index += 1;
            let (contents, next) = parse_add_body(&lines, index)?;
            hunks.push(Hunk::Add {
                path: path.to_string(),
                contents,
            });
            index = next;
            continue;
        }
        if let Some(path) = trimmed.strip_prefix(DELETE_FILE_MARKER) {
            hunks.push(Hunk::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }
        if let Some(path) = trimmed.strip_prefix(UPDATE_FILE_MARKER) {
            let header_line = line_number;
            index += 1;
            let (move_path, chunks, next) = parse_update_body(&lines, index, path, header_line)?;
            hunks.push(Hunk::Update {
                path: path.to_string(),
                move_path,
                chunks,
            });
            index = next;
            continue;
        }
        return Err(invalid_header(trimmed, line_number));
    }

    if !seen_end {
        return Err(ParseError::InvalidPatch(
            "The last line of the patch must be '*** End Patch'".into(),
        ));
    }

    // Allow only blank lines after the end marker.
    while index < lines.len() {
        if !lines[index].trim().is_empty() {
            return Err(ParseError::InvalidPatch(
                "The last line of the patch must be '*** End Patch'".into(),
            ));
        }
        index += 1;
    }

    if hunks.is_empty() {
        return Err(ParseError::InvalidPatch(
            "patch must contain at least one file operation".into(),
        ));
    }
    Ok(hunks)
}

fn parse_add_body(lines: &[&str], mut index: usize) -> Result<(String, usize), ParseError> {
    let mut contents = String::new();
    while index < lines.len() {
        let raw = lines[index];
        let trimmed = raw.trim();
        if is_file_header_or_end(trimmed) {
            break;
        }
        let Some(added) = raw.strip_prefix('+') else {
            return Err(ParseError::InvalidHunk {
                message: format!("Add File lines must start with '+', got: '{raw}'"),
                line_number: index + 1,
            });
        };
        contents.push_str(added);
        contents.push('\n');
        index += 1;
    }
    Ok((contents, index))
}

fn parse_update_body(
    lines: &[&str],
    mut index: usize,
    path: &str,
    header_line: usize,
) -> Result<(Option<String>, Vec<UpdateFileChunk>, usize), ParseError> {
    let mut move_path = None;
    let mut chunks = Vec::new();

    if index < lines.len() {
        let trimmed = lines[index].trim_end();
        if let Some(dest) = trimmed.strip_prefix(MOVE_TO_MARKER) {
            move_path = Some(dest.to_string());
            index += 1;
        }
    }

    while index < lines.len() {
        let raw = lines[index];
        let trimmed = raw.trim_end();
        if is_file_header_or_end(trimmed.trim()) {
            break;
        }
        let (chunk, next) = parse_chunk(lines, index)?;
        chunks.push(chunk);
        index = next;
    }

    if chunks.is_empty() {
        return Err(ParseError::InvalidHunk {
            message: format!("Update file hunk for path '{path}' is empty"),
            line_number: header_line,
        });
    }
    Ok((move_path, chunks, index))
}

fn parse_chunk(lines: &[&str], mut index: usize) -> Result<(UpdateFileChunk, usize), ParseError> {
    let start_line = index + 1;
    let intro = lines[index].trim_end();
    let change_context = if intro == EMPTY_CHANGE_CONTEXT_MARKER {
        index += 1;
        None
    } else if let Some(ctx) = intro.strip_prefix(CHANGE_CONTEXT_MARKER) {
        index += 1;
        Some(ctx.to_string())
    } else if intro.is_empty()
        || intro.starts_with(' ')
        || intro.starts_with('+')
        || intro.starts_with('-')
    {
        // Chunk may start directly with body lines (implicit empty @@).
        None
    } else {
        return Err(ParseError::InvalidHunk {
            message: format!(
                "Expected update hunk to start with a @@ context marker, got: '{}'",
                lines[index]
            ),
            line_number: start_line,
        });
    };

    let mut chunk = UpdateFileChunk {
        change_context,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        is_end_of_file: false,
    };

    while index < lines.len() {
        let raw = lines[index];
        let trimmed_end = raw.trim_end();
        let trimmed = trimmed_end.trim();

        if chunk.is_end_of_file {
            if trimmed_end.is_empty() {
                index += 1;
                continue;
            }
            // Next chunk or file header ends this chunk.
            break;
        }

        if is_file_header_or_end(trimmed) {
            break;
        }
        if trimmed_end == EMPTY_CHANGE_CONTEXT_MARKER
            || trimmed_end.starts_with(CHANGE_CONTEXT_MARKER)
        {
            if chunk.is_empty() {
                return Err(ParseError::InvalidHunk {
                    message: format!(
                        "Unexpected line found in update hunk: '{raw}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                    ),
                    line_number: index + 1,
                });
            }
            break;
        }
        if trimmed_end == EOF_MARKER {
            if chunk.is_empty() {
                return Err(ParseError::InvalidHunk {
                    message: "Update hunk does not contain any lines".into(),
                    line_number: index + 1,
                });
            }
            chunk.is_end_of_file = true;
            index += 1;
            continue;
        }

        if raw.is_empty() {
            chunk.old_lines.push(String::new());
            chunk.new_lines.push(String::new());
            index += 1;
            continue;
        }
        if let Some(content) = raw.strip_prefix(' ') {
            chunk.old_lines.push(content.to_string());
            chunk.new_lines.push(content.to_string());
            index += 1;
            continue;
        }
        if let Some(content) = raw.strip_prefix('+') {
            chunk.new_lines.push(content.to_string());
            index += 1;
            continue;
        }
        if let Some(content) = raw.strip_prefix('-') {
            chunk.old_lines.push(content.to_string());
            index += 1;
            continue;
        }

        if !chunk.is_empty() {
            return Err(ParseError::InvalidHunk {
                message: format!(
                    "Expected update hunk to start with a @@ context marker, got: '{raw}'"
                ),
                line_number: index + 1,
            });
        }
        return Err(ParseError::InvalidHunk {
            message: format!(
                "Unexpected line found in update hunk: '{raw}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
            ),
            line_number: index + 1,
        });
    }

    if chunk.is_empty() {
        return Err(ParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".into(),
            line_number: start_line,
        });
    }
    Ok((chunk, index))
}

fn is_file_header_or_end(trimmed: &str) -> bool {
    trimmed == END_PATCH_MARKER
        || trimmed.starts_with(ADD_FILE_MARKER)
        || trimmed.starts_with(DELETE_FILE_MARKER)
        || trimmed.starts_with(UPDATE_FILE_MARKER)
}

fn invalid_header(trimmed: &str, line_number: usize) -> ParseError {
    ParseError::InvalidHunk {
        message: format!(
            "'{trimmed}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
        ),
        line_number,
    }
}

fn patch_lines(patch: &str) -> Vec<&str> {
    if patch.is_empty() {
        return Vec::new();
    }
    patch
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}
