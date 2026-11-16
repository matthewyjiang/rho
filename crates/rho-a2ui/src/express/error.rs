use std::fmt;

use thiserror::Error;

/// A place in an Express block: a 1-based line, and a 1-based column counted
/// in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    /// The line, from 1.
    pub line: u32,
    /// The column, from 1, in characters.
    pub column: u32,
}

/// What kind of failure an [`ExpressError`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The text is not Express: a syntax error, an unterminated string, a
    /// number out of range, a block with nothing in it.
    Parse,
    /// The text is Express, but the catalogs refuse what it says: an unknown
    /// component, property or function, a value its schema rejects, a
    /// duplicate component id. A decompiled message the notation cannot
    /// express fails this way too.
    Validation,
    /// The catalogs themselves cannot be used together: none at all, two
    /// with one id, or one that does not target protocol v1.0.
    Catalog,
}

/// Why an Express block did not compile, or messages did not decompile.
///
/// The message is written for the model that wrote the block: it names what
/// is wrong and, where it can, what the catalog accepts instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub struct ExpressError {
    kind: ErrorKind,
    message: String,
    position: Option<Position>,
}

impl ExpressError {
    pub(crate) fn parse(position: Position, message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Parse,
            message: message.into(),
            position: Some(position),
        }
    }

    pub(crate) fn validation(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Validation,
            message: message.into(),
            position: None,
        }
    }

    pub(crate) fn catalog(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Catalog,
            message: message.into(),
            position: None,
        }
    }

    /// The error with `position`, unless it already has one: the innermost
    /// construct that failed is the one a reader needs.
    #[must_use]
    pub(crate) fn at(mut self, position: Position) -> Self {
        self.position.get_or_insert(position);
        self
    }

    /// What kind of failure this is.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Where in the block it happened, when it is tied to a place.
    pub fn position(&self) -> Option<Position> {
        self.position
    }

    /// What went wrong, without the position.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ExpressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some(Position { line, column }) => {
                write!(f, "line {line}, column {column}: {}", self.message)
            }
            None => f.write_str(&self.message),
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.column)
    }
}
