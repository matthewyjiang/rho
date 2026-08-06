use std::ops::Range;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("invalid token at byte {byte}")]
    InvalidToken { byte: usize },

    #[error("unexpected end of input")]
    UnexpectedEof,

    #[error("unexpected token at position {position}: {token}")]
    UnexpectedToken { position: usize, token: String },

    #[error("unexpected whitespace")]
    UnexpectedWhitespace,

    #[error("unexpected \\right without matching \\left")]
    UnexpectedRightWithoutLeft,

    #[error("expected {expected}")]
    ExpectedToken { expected: String },

    #[error("expected {expected}, got {got}")]
    ExpectedTokenGot { expected: String, got: String },

    #[error("expected a string")]
    ExpectedString,

    #[error("expected a delimiter after \\{side}")]
    ExpectedDelimiter { side: &'static str },

    #[error("mismatched delimiters: \\left{left} and \\right{right}")]
    MismatchedDelimiters { left: char, right: char },

    #[error("unclosed \\left ... \\right pair")]
    UnclosedLeftRight,

    #[error("unclosed \\begin{{{name}}}")]
    UnclosedEnvironment { name: String },

    #[error("mismatched \\begin{{{begin}}} and \\end{{{end}}}")]
    MismatchedEnvironment { begin: String, end: String },

    #[error("unknown matrix environment: {name}")]
    UnknownEnvironment { name: String },

    #[error("expected a color name")]
    ExpectedColorName,

    #[error("invalid color name: {name}")]
    InvalidColor { name: String },

    #[error("matrix rows have different lengths")]
    MismatchedMatrixRows,

    #[error("internal parser error: {0}")]
    Internal(String),

    #[error("{msg} near '{snippet}' at byte {byte}")]
    Located {
        msg: String,
        snippet: String,
        byte: usize,
    },

    #[error("{msg} but reached end of input near '{snippet}'")]
    LocatedEof { msg: String, snippet: String },
}

impl ParseError {
    pub fn at(self, span: Range<usize>, input: &str) -> Self {
        let snippet = input[span.clone()].to_string();
        ParseError::Located {
            msg: self.to_string(),
            snippet,
            byte: span.start,
        }
    }

    pub fn at_eof(self, input: &str) -> Self {
        if input.is_empty() {
            return self;
        }

        let n = input.len();
        let snippet = input[n.saturating_sub(5)..].to_string();
        ParseError::LocatedEof {
            msg: self.to_string(),
            snippet,
        }
    }
}
