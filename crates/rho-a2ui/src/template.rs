use std::collections::BTreeMap;

use serde_json::Value;

use crate::coerce::number_value;
use crate::expr::{Call, Expr, ExpressionError};

/// The deepest an expression may nest: interpolations inside interpolations,
/// call arguments, and wire containers all count. Every A2UI engine shares
/// this bound, so a payload parses everywhere or nowhere.
pub const MAX_DEPTH: usize = 100;

/// The longest template, in characters, that [`parse_template`] accepts.
pub const MAX_TEMPLATE_CHARS: usize = 10_000;

/// The most parts a template may have.
pub const MAX_TEMPLATE_PARTS: usize = 1_000;

/// Parses a `formatString` template into its parts.
///
/// Text outside `${...}` is a literal, and `\${` is the literal `${`. Inside,
/// an expression is a quoted string (`\n`, `\t` and `\r` escapes, any other
/// escaped character stands for itself), a number, `true`, `false`, `null`
/// (an empty string), a nested `${...}`, a call `name(arg: value, ...)`, or
/// else a data binding path. Paths and names are made of UAX 31
/// `XID_Continue` characters plus `/`, `.` and `-` in paths. Adjacent string
/// literals are joined and empty ones dropped, so `a${null}b` is one part.
///
/// # Errors
///
/// Fails on malformed syntax, on a template over [`MAX_TEMPLATE_CHARS`]
/// characters or [`MAX_TEMPLATE_PARTS`] parts, and on nesting deeper than
/// [`MAX_DEPTH`].
pub fn parse_template(template: &str) -> Result<Vec<Expr>, ExpressionError> {
    let chars = template.chars().count();
    if chars > MAX_TEMPLATE_CHARS {
        return Err(ExpressionError::TemplateTooLong { chars });
    }
    let mut parts = Parts::default();
    let mut scanner = Scanner::new(template);
    while !scanner.at_end() {
        if parts.0.len() >= MAX_TEMPLATE_PARTS {
            return Err(ExpressionError::TooManyParts);
        }
        if scanner.eat("${") {
            let content = scanner.interpolation()?;
            parts.push(parse_expression(content, 1)?);
        } else if scanner.eat("\\${") {
            parts.push(Expr::Literal(Value::String("${".to_owned())));
        } else {
            let start = scanner.pos;
            while !scanner.at_end() && !scanner.sees("${") && !scanner.sees("\\${") {
                scanner.bump();
            }
            parts.push(Expr::Literal(Value::String(
                template[start..scanner.pos].to_owned(),
            )));
        }
    }
    Ok(parts.0)
}

/// Template parts with adjacent string literals joined and empty ones dropped.
#[derive(Default)]
struct Parts(Vec<Expr>);

impl Parts {
    fn push(&mut self, part: Expr) {
        if let Expr::Literal(Value::String(text)) = &part {
            if text.is_empty() {
                return;
            }
            if let Some(Expr::Literal(Value::String(last))) = self.0.last_mut() {
                last.push_str(text);
                return;
            }
        }
        self.0.push(part);
    }
}

/// Parses the text between `${` and `}` as one expression, at `depth`.
fn parse_expression(content: &str, depth: usize) -> Result<Expr, ExpressionError> {
    let mut scanner = Scanner::new(content.trim_matches(is_whitespace));
    let expr = parse_value(&mut scanner, depth)?;
    scanner.skip_whitespace();
    if !scanner.at_end() {
        return Err(ExpressionError::UnexpectedCharacters {
            rest: scanner.rest().to_owned(),
        });
    }
    Ok(expr)
}

fn parse_value(scanner: &mut Scanner<'_>, depth: usize) -> Result<Expr, ExpressionError> {
    if depth > MAX_DEPTH {
        return Err(ExpressionError::TooDeep);
    }
    scanner.skip_whitespace();
    if scanner.at_end() {
        return Ok(empty_string());
    }
    if scanner.eat("${") {
        let content = scanner.interpolation()?;
        return parse_expression(content, depth + 1);
    }
    if let Some(quote @ ('\'' | '"')) = scanner.peek() {
        return scanner.string_literal(quote);
    }
    if scanner.sees_number() {
        return scanner.number_literal();
    }
    if scanner.eat_keyword("true") {
        return Ok(Expr::Literal(Value::Bool(true)));
    }
    if scanner.eat_keyword("false") {
        return Ok(Expr::Literal(Value::Bool(false)));
    }
    if scanner.eat_keyword("null") {
        return Ok(empty_string());
    }
    let token = scanner.path_or_name()?;
    scanner.skip_whitespace();
    if scanner.peek() == Some('(') {
        if token.contains('~') {
            return Err(ExpressionError::TildeInFunctionName {
                name: token.to_owned(),
            });
        }
        return parse_call(token, scanner, depth);
    }
    if token.starts_with('@') {
        return Err(ExpressionError::SystemNameWithoutCall {
            name: token.to_owned(),
        });
    }
    if token.is_empty() {
        return Ok(empty_string());
    }
    Ok(Expr::Path(token.to_owned()))
}

fn parse_call(
    name: &str,
    scanner: &mut Scanner<'_>,
    depth: usize,
) -> Result<Expr, ExpressionError> {
    scanner.eat("(");
    scanner.skip_whitespace();
    let mut args = BTreeMap::new();
    while !scanner.at_end() && scanner.peek() != Some(')') {
        let argument = scanner.identifier();
        scanner.skip_whitespace();
        if !scanner.eat(":") {
            return Err(ExpressionError::MissingArgumentColon {
                function: name.to_owned(),
                argument: argument.to_owned(),
            });
        }
        let value = parse_value(scanner, depth + 1)?;
        args.insert(argument.to_owned(), value);
        scanner.skip_whitespace();
        if scanner.eat(",") {
            scanner.skip_whitespace();
        }
    }
    if !scanner.eat(")") {
        return Err(ExpressionError::UnclosedCall {
            function: name.to_owned(),
        });
    }
    Ok(Expr::Call(Call {
        name: name.to_owned(),
        args,
        catalog_id: None,
    }))
}

fn empty_string() -> Expr {
    Expr::Literal(Value::String(String::new()))
}

/// The whitespace allowed around expressions and arguments.
fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_identifier(c: char) -> bool {
    unicode_ident::is_xid_continue(c)
}

/// Parses `text` if it is a whole number literal: an optional sign, digits
/// with at most one `.` (`1.`, `.5` and `007` included), and an optional
/// exponent with at least one digit. The result may be infinite.
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok =
        digits(whole) && digits(fraction) && !(whole.is_empty() && fraction.is_empty());
    let exponent_ok = exponent.is_none_or(|e| {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        !e.is_empty() && digits(e)
    });
    if !(mantissa_ok && exponent_ok) {
        return None;
    }
    text.parse().ok()
}

struct Scanner<'s> {
    input: &'s str,
    pos: usize,
}

impl<'s> Scanner<'s> {
    fn new(input: &'s str) -> Self {
        Self { input, pos: 0 }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn rest(&self) -> &'s str {
        &self.input[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn sees(&self, prefix: &str) -> bool {
        self.rest().starts_with(prefix)
    }

    fn eat(&mut self, prefix: &str) -> bool {
        let found = self.sees(prefix);
        if found {
            self.pos += prefix.len();
        }
        found
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(is_whitespace) {
            self.bump();
        }
    }

    /// A keyword, when no identifier character follows it.
    fn eat_keyword(&mut self, keyword: &str) -> bool {
        let follows = self
            .rest()
            .get(keyword.len()..)
            .and_then(|s| s.chars().next());
        if self.sees(keyword) && !follows.is_some_and(is_identifier) {
            self.pos += keyword.len();
            return true;
        }
        false
    }

    /// Reads up to the `}` closing an interpolation already opened, skipping
    /// braces inside quoted strings, and returns what lies between.
    fn interpolation(&mut self) -> Result<&'s str, ExpressionError> {
        let start = self.pos;
        let mut open = 1;
        while open > 0 {
            match self.bump() {
                None => return Err(ExpressionError::UnclosedInterpolation),
                Some('{') => open += 1,
                Some('}') => open -= 1,
                Some(quote @ ('\'' | '"')) => loop {
                    match self.bump() {
                        None => return Err(ExpressionError::UnclosedString),
                        Some('\\') => {
                            self.bump().ok_or(ExpressionError::UnclosedString)?;
                        }
                        Some(c) if c == quote => break,
                        Some(_) => {}
                    }
                },
                Some(_) => {}
            }
        }
        Ok(&self.input[start..self.pos - 1])
    }

    fn string_literal(&mut self, quote: char) -> Result<Expr, ExpressionError> {
        self.bump();
        let mut text = String::new();
        loop {
            match self.bump() {
                None => return Err(ExpressionError::UnclosedString),
                Some('\\') => match self.bump() {
                    None => return Err(ExpressionError::UnclosedString),
                    Some('n') => text.push('\n'),
                    Some('t') => text.push('\t'),
                    Some('r') => text.push('\r'),
                    Some(other) => text.push(other),
                },
                Some(c) if c == quote => return Ok(Expr::Literal(Value::String(text))),
                Some(c) => text.push(c),
            }
        }
    }

    /// Whether a number starts here: a digit, or `.` and a digit, after an
    /// optional sign. So `-a`, `.foo` and `-.` are paths.
    fn sees_number(&self) -> bool {
        let skip = usize::from(matches!(self.peek(), Some('+' | '-')));
        let digit = |n: usize| self.peek_nth(n).is_some_and(|c| c.is_ascii_digit());
        digit(skip) || (self.peek_nth(skip) == Some('.') && digit(skip + 1))
    }

    fn number_literal(&mut self) -> Result<Expr, ExpressionError> {
        let start = self.pos;
        if matches!(self.peek(), Some('+' | '-')) {
            self.bump();
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.bump();
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        let literal = &self.input[start..self.pos];
        match parse_number(literal) {
            None => Err(ExpressionError::InvalidNumber {
                literal: literal.to_owned(),
            }),
            Some(number) if !number.is_finite() => Err(ExpressionError::NumberOutOfRange {
                literal: literal.to_owned(),
            }),
            Some(number) => Ok(Expr::Literal(number_value(number))),
        }
    }

    /// A path, or a function name: `@` and ASCII word characters for a
    /// system function, otherwise identifier characters, `/`, `.`, `-`, and
    /// `~0` or `~1` escapes.
    fn path_or_name(&mut self) -> Result<&'s str, ExpressionError> {
        let start = self.pos;
        if self.peek() == Some('@') {
            if !self
                .peek_nth(1)
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            {
                return Err(ExpressionError::InvalidSystemName);
            }
            self.bump();
            while self
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                self.bump();
            }
            return Ok(&self.input[start..self.pos]);
        }
        while let Some(c) = self.peek() {
            if is_identifier(c) || matches!(c, '/' | '.' | '-') {
                self.bump();
            } else if c == '~' {
                if !matches!(self.peek_nth(1), Some('0' | '1')) {
                    return Err(ExpressionError::InvalidPathEscape {
                        path: self.input[start..self.pos].to_owned(),
                    });
                }
                self.pos += 2;
            } else {
                break;
            }
        }
        Ok(&self.input[start..self.pos])
    }

    fn identifier(&mut self) -> &'s str {
        let start = self.pos;
        while self.peek().is_some_and(is_identifier) {
            self.bump();
        }
        &self.input[start..self.pos]
    }
}
