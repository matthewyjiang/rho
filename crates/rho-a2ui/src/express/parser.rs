//! A hand-written recursive-descent parser for `Express.g4`, the grammar in
//! `specification/inference_formats/express/` upstream. A change to the
//! grammar has to be made here too; the compile and decompile suites check
//! that the two still agree.

use serde_json::Value;

use super::error::{ExpressError, Position};
use super::lexer::{literal_value, tokenize, Token, TokenKind};
use crate::template::MAX_DEPTH;

/// One statement of a block.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Statement {
    pub(crate) pos: Position,
    pub(crate) kind: StatementKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StatementKind {
    /// `name = value`, or `$/path = value` when `target` is a path.
    Assign { target: Target, value: Node },
    /// A standalone expression, such as `surface("main")` or a renderer
    /// function call.
    Expr(Node),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Target {
    Variable(String),
    /// The text after the `$`.
    Path(String),
}

/// An expression, with where it starts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub(crate) pos: Position,
    pub(crate) kind: NodeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NodeKind {
    /// A string, number, boolean or `null`.
    Literal(Value),
    Array(Vec<Node>),
    /// Keys as written; a repeated key keeps its first place and last value.
    Map(Vec<(String, Node)>),
    /// The text after the `$`.
    Path(String),
    Variable(String),
    /// `_`, which skips an argument.
    Skipped,
    Check(Check),
    Call(Call),
}

/// `?name` or `?name(args)`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Check {
    pub(crate) name: String,
    pub(crate) args: Vec<Node>,
}

/// `name(args, key=value)`: a component, a function, or one of the
/// reserved forms `surface`, `deleteSurface`, `Event` and `_template`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Call {
    pub(crate) name: String,
    pub(crate) args: Vec<Node>,
    /// Keyword arguments in the order written. No keyword repeats.
    pub(crate) kwargs: Vec<(String, Node)>,
}

impl Call {
    pub(crate) fn kwarg(&self, key: &str) -> Option<&Node> {
        self.kwargs
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value))
    }

    /// The argument given by keyword `key`, else at position `index`.
    pub(crate) fn arg(&self, index: usize, key: &str) -> Option<&Node> {
        self.kwarg(key).or_else(|| self.args.get(index))
    }
}

/// Parses a block into statements.
///
/// # Errors
///
/// A parse error at the first token the grammar does not admit.
pub(crate) fn parse(source: &str) -> Result<Vec<Statement>, ExpressError> {
    let tokens = tokenize(source)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        end: end_position(source),
    };
    let mut statements = Vec::new();
    while parser.peek().is_some() {
        statements.push(parser.statement()?);
    }
    Ok(statements)
}

/// Where the block ends, for errors at its end.
fn end_position(source: &str) -> Position {
    let line = source.split('\n').count();
    let last = source.rsplit('\n').next().unwrap_or_default();
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        column: u32::try_from(last.chars().count() + 1).unwrap_or(u32::MAX),
    }
}

struct Parser<'t> {
    tokens: &'t [Token],
    at: usize,
    end: Position,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&TokenKind> {
        self.tokens.get(self.at).map(|t| &t.kind)
    }

    fn peek_second(&self) -> Option<&TokenKind> {
        self.tokens.get(self.at + 1).map(|t| &t.kind)
    }

    fn pos(&self) -> Position {
        self.tokens.get(self.at).map_or(self.end, |t| t.pos)
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        let found = self.peek() == Some(kind);
        if found {
            self.at += 1;
        }
        found
    }

    fn found(&self) -> String {
        self.peek()
            .map_or_else(|| "the end of the block".to_owned(), TokenKind::describe)
    }

    fn error(&self, expected: &str) -> ExpressError {
        ExpressError::parse(
            self.pos(),
            format!("expected {expected}, found {}", self.found()),
        )
    }

    fn statement(&mut self) -> Result<Statement, ExpressError> {
        let pos = self.pos();
        let target = match (self.peek(), self.peek_second()) {
            (Some(TokenKind::Ident(name)), Some(TokenKind::Equals)) => {
                Some(Target::Variable(name.clone()))
            }
            (Some(TokenKind::Path(path)), Some(TokenKind::Equals)) => {
                Some(Target::Path(path.clone()))
            }
            _ => None,
        };
        let kind = match target {
            Some(target) => {
                self.at += 2;
                StatementKind::Assign {
                    target,
                    value: self.expression(0)?,
                }
            }
            None => StatementKind::Expr(self.expression(0)?),
        };
        Ok(Statement { pos, kind })
    }

    fn expression(&mut self, depth: usize) -> Result<Node, ExpressError> {
        let pos = self.pos();
        if depth > MAX_DEPTH {
            return Err(ExpressError::parse(
                pos,
                format!("expressions nest deeper than {MAX_DEPTH} levels"),
            ));
        }
        let Some(token) = self.peek() else {
            return Err(self.error("an expression"));
        };
        let kind = match token {
            TokenKind::LBracket => {
                self.at += 1;
                NodeKind::Array(
                    self.list(&TokenKind::RBracket, "']'", |p| p.expression(depth + 1))?,
                )
            }
            TokenKind::LBrace => {
                self.at += 1;
                NodeKind::Map(self.list(&TokenKind::RBrace, "'}'", |p| p.map_entry(depth + 1))?)
            }
            TokenKind::Path(path) => {
                let path = path.clone();
                self.at += 1;
                NodeKind::Path(path)
            }
            TokenKind::Check(name) => {
                let name = name.clone();
                self.at += 1;
                let args = if self.eat(&TokenKind::LParen) {
                    self.list(&TokenKind::RParen, "')'", |p| p.expression(depth + 1))?
                } else {
                    Vec::new()
                };
                NodeKind::Check(Check { name, args })
            }
            TokenKind::Ident(name) | TokenKind::System(name) => {
                let name = name.clone();
                let system = name.starts_with('@');
                self.at += 1;
                if self.eat(&TokenKind::LParen) {
                    NodeKind::Call(self.call(name, depth)?)
                } else if system {
                    return Err(self.error(&format!("'(' after the system function {name}")));
                } else {
                    NodeKind::Variable(name)
                }
            }
            TokenKind::Underscore => {
                self.at += 1;
                NodeKind::Skipped
            }
            other => match literal_value(other) {
                Some(value) => {
                    self.at += 1;
                    NodeKind::Literal(value)
                }
                None => return Err(self.error("an expression")),
            },
        };
        Ok(Node { pos, kind })
    }

    /// Items up to `close`, separated by commas, with an optional trailing
    /// comma. The opening bracket is already read.
    fn list<T>(
        &mut self,
        close: &TokenKind,
        close_text: &str,
        mut item: impl FnMut(&mut Self) -> Result<T, ExpressError>,
    ) -> Result<Vec<T>, ExpressError> {
        let mut items = Vec::new();
        loop {
            if self.eat(close) {
                return Ok(items);
            }
            items.push(item(self)?);
            if self.eat(close) {
                return Ok(items);
            }
            if !self.eat(&TokenKind::Comma) {
                return Err(self.error(&format!("',' or {close_text}")));
            }
        }
    }

    fn map_entry(&mut self, depth: usize) -> Result<(String, Node), ExpressError> {
        let key = match self.peek() {
            Some(TokenKind::Ident(key) | TokenKind::Str(key)) => key.clone(),
            _ => return Err(self.error("a map key, an identifier or a string")),
        };
        self.at += 1;
        if !self.eat(&TokenKind::Colon) {
            return Err(self.error(&format!("':' after the map key {key:?}")));
        }
        Ok((key, self.expression(depth)?))
    }

    /// The arguments of a call to `name`, whose `(` is already read.
    fn call(&mut self, name: String, depth: usize) -> Result<Call, ExpressError> {
        let mut call = Call {
            name,
            args: Vec::new(),
            kwargs: Vec::new(),
        };
        let close_text = format!("')' to close the call to {}", call.name);
        let arguments = self.list(&TokenKind::RParen, &close_text, |p| {
            let keyword = match (p.peek(), p.peek_second()) {
                (Some(TokenKind::Ident(key)), Some(TokenKind::Equals)) => Some(key.clone()),
                _ => None,
            };
            if let Some(key) = keyword {
                let pos = p.pos();
                p.at += 2;
                Ok((Some((key, pos)), p.expression(depth + 1)?))
            } else {
                Ok((None, p.expression(depth + 1)?))
            }
        })?;
        for (keyword, value) in arguments {
            match keyword {
                None => call.args.push(value),
                Some((key, pos)) => {
                    if call.kwarg(&key).is_some() {
                        return Err(ExpressError::parse(
                            pos,
                            format!(
                                "the keyword argument {key} is given twice in the call to {}",
                                call.name
                            ),
                        ));
                    }
                    call.kwargs.push((key, value));
                }
            }
        }
        Ok(call)
    }
}

impl Statement {
    /// A standalone call, the only kind of expression statement that does
    /// anything.
    pub(crate) fn call(&self) -> Option<&Call> {
        match &self.kind {
            StatementKind::Expr(Node {
                kind: NodeKind::Call(call),
                ..
            }) => Some(call),
            _ => None,
        }
    }
}

impl Node {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Literal(Value::String(text)) => Some(text),
            _ => None,
        }
    }

    /// How the value reads in an error message, in the model's terms.
    pub(crate) fn describe(&self) -> String {
        match &self.kind {
            NodeKind::Call(call) => format!("a call to {}(...)", call.name),
            NodeKind::Check(check) => format!("a check (?{})", check.name),
            NodeKind::Variable(name) => format!("the variable {name}"),
            NodeKind::Path(path) => format!("the data binding ${path}"),
            NodeKind::Skipped => "'_'".to_owned(),
            NodeKind::Map(_) => "a map literal".to_owned(),
            NodeKind::Array(_) => "a list".to_owned(),
            NodeKind::Literal(Value::Null) => "null".to_owned(),
            NodeKind::Literal(Value::String(text)) => format!("the string {text:?}"),
            NodeKind::Literal(value) => format!("the value {value}"),
        }
    }
}
