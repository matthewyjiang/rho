//! Express tokens, as `Express.g4` defines them, with the repairs the
//! compile suite tolerates on top of the grammar: `--` line comments, raw
//! line breaks at the edges of a standard string, data paths and
//! identifiers broken across a line, and `<a2ui>` tags around the block.

use std::collections::HashSet;

use serde_json::{Number, Value};

use super::error::{ExpressError, Position};

/// The longest block, in bytes, the compiler reads.
pub(crate) const MAX_BLOCK_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    Ident(String),
    /// `@name`, a system function, with its `@`.
    System(String),
    /// `$...`, without the `$`.
    Path(String),
    /// `?name`, without the `?`.
    Check(String),
    Number(Number),
    Str(String),
    Bool(bool),
    Null,
    Underscore,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Equals,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Token {
    pub(crate) kind: TokenKind,
    pub(crate) pos: Position,
    /// The line breaks between this token and the one before it.
    newlines_before: u32,
    /// Whether only spaces, tabs and line breaks separate this token from
    /// the one before it: no comment and no `;`.
    clean_gap: bool,
}

impl TokenKind {
    /// How the token reads in an error message.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Ident(name) | Self::System(name) => format!("'{name}'"),
            Self::Path(path) => format!("'${path}'"),
            Self::Check(name) => format!("'?{name}'"),
            Self::Number(number) => format!("the number {number}"),
            Self::Str(_) => "a string".to_owned(),
            Self::Bool(value) => format!("'{value}'"),
            Self::Null => "'null'".to_owned(),
            Self::Underscore => "'_'".to_owned(),
            Self::LParen => "'('".to_owned(),
            Self::RParen => "')'".to_owned(),
            Self::LBracket => "'['".to_owned(),
            Self::RBracket => "']'".to_owned(),
            Self::LBrace => "'{'".to_owned(),
            Self::RBrace => "'}'".to_owned(),
            Self::Comma => "','".to_owned(),
            Self::Colon => "':'".to_owned(),
            Self::Equals => "'='".to_owned(),
        }
    }
}

/// Splits `source` into tokens.
///
/// # Errors
///
/// A [`ErrorKind::Parse`](super::ErrorKind::Parse) error for a character no
/// token admits, an unterminated string or comment, a number out of the
/// range of a double, or a block over [`MAX_BLOCK_BYTES`].
pub(crate) fn tokenize(source: &str) -> Result<Vec<Token>, ExpressError> {
    if source.len() > MAX_BLOCK_BYTES {
        return Err(ExpressError::parse(
            Position { line: 1, column: 1 },
            format!("the block is longer than {MAX_BLOCK_BYTES} bytes"),
        ));
    }
    let stripped = strip_sentinels(source);
    let mut lexer = Lexer::new(stripped.as_deref().unwrap_or(source));
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token()? {
        tokens.push(token);
    }
    join_split_identifiers(&mut tokens);
    Ok(tokens)
}

struct Lexer<'s> {
    text: &'s str,
    at: usize,
    line: u32,
    column: u32,
    newlines: u32,
    clean: bool,
}

impl<'s> Lexer<'s> {
    fn new(text: &'s str) -> Self {
        Self {
            text,
            at: 0,
            line: 1,
            column: 1,
            newlines: 0,
            clean: true,
        }
    }

    fn rest(&self) -> &'s str {
        &self.text[self.at..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.rest().chars().nth(n)
    }

    fn pos(&self) -> Position {
        Position {
            line: self.line,
            column: self.column,
        }
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        if c == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    fn bump_n(&mut self, n: usize) {
        for _ in 0..n {
            self.bump();
        }
    }

    fn eat_while(&mut self, keep: impl Fn(char) -> bool) -> &'s str {
        let start = self.at;
        while self.peek().is_some_and(&keep) {
            self.bump();
        }
        &self.text[start..self.at]
    }

    /// Skips whitespace, `;` and comments, noting what the gap held.
    fn skip_trivia(&mut self) -> Result<(), ExpressError> {
        loop {
            let rest = self.rest();
            match self.peek() {
                Some('\n') => {
                    self.newlines += 1;
                    self.bump();
                }
                Some(' ' | '\t' | '\r') => {
                    self.bump();
                }
                Some(';') => {
                    self.clean = false;
                    self.bump();
                }
                Some('#') => self.line_comment(),
                _ if rest.starts_with("//") || rest.starts_with("--") => self.line_comment(),
                _ if rest.starts_with("/*") => {
                    let start = self.pos();
                    self.clean = false;
                    let Some(end) = rest[2..].find("*/") else {
                        return Err(ExpressError::parse(start, "unterminated block comment"));
                    };
                    let comment = &rest[..end + 4];
                    self.newlines += count_newlines(comment);
                    self.bump_n(comment.chars().count());
                }
                _ => return Ok(()),
            }
        }
    }

    fn line_comment(&mut self) {
        self.clean = false;
        self.eat_while(|c| c != '\n');
    }

    fn next_token(&mut self) -> Result<Option<Token>, ExpressError> {
        self.skip_trivia()?;
        let pos = self.pos();
        let Some(c) = self.peek() else {
            return Ok(None);
        };
        let kind = match c {
            '(' => self.punct(TokenKind::LParen),
            ')' => self.punct(TokenKind::RParen),
            '[' => self.punct(TokenKind::LBracket),
            ']' => self.punct(TokenKind::RBracket),
            '{' => self.punct(TokenKind::LBrace),
            '}' => self.punct(TokenKind::RBrace),
            ',' => self.punct(TokenKind::Comma),
            ':' => self.punct(TokenKind::Colon),
            '=' => self.punct(TokenKind::Equals),
            '"' => self.string(pos)?,
            'r' | 'R' if self.peek_at(1) == Some('"') => match self.raw_string(pos)? {
                Some(kind) => kind,
                None => self.word(),
            },
            '$' => self.path(),
            '?' | '@' => self.sigil(c, pos)?,
            _ if self.sees_number() => self.number(pos)?,
            _ if c == '_' || unicode_ident::is_xid_start(c) => self.word(),
            _ => {
                return Err(ExpressError::parse(
                    pos,
                    format!("unexpected character {c:?}"),
                ))
            }
        };
        let token = Token {
            kind,
            pos,
            newlines_before: self.newlines,
            clean_gap: self.clean,
        };
        self.newlines = 0;
        self.clean = true;
        Ok(Some(token))
    }

    fn punct(&mut self, kind: TokenKind) -> TokenKind {
        self.bump();
        kind
    }

    /// An identifier, a keyword, or `_`.
    fn word(&mut self) -> TokenKind {
        let first = self.bump().unwrap_or_default();
        let start = self.at - first.len_utf8();
        self.eat_while(unicode_ident::is_xid_continue);
        match &self.text[start..self.at] {
            "_" => TokenKind::Underscore,
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "null" => TokenKind::Null,
            word => TokenKind::Ident(word.to_owned()),
        }
    }

    /// `?name` or `@name`.
    fn sigil(&mut self, sigil: char, pos: Position) -> Result<TokenKind, ExpressError> {
        let starts_name = self
            .peek_at(1)
            .is_some_and(|c| c == '_' || unicode_ident::is_xid_start(c));
        if !starts_name {
            let what = if sigil == '?' {
                "a check name"
            } else {
                "a system function name"
            };
            return Err(ExpressError::parse(
                pos,
                format!("expected {what} after '{sigil}'"),
            ));
        }
        self.bump();
        let name = self.eat_while(unicode_ident::is_xid_continue);
        Ok(if sigil == '?' {
            TokenKind::Check(name.to_owned())
        } else {
            TokenKind::System(format!("@{name}"))
        })
    }

    /// `$` and the path after it. A path that ends in `/` at the end of a
    /// line continues on the next line, unless that line starts an
    /// assignment.
    fn path(&mut self) -> TokenKind {
        self.bump();
        let mut path = self.eat_while(is_path_char).to_owned();
        if path.ends_with('/') {
            if let Some((skip, rest)) = split_path_continuation(self.rest()) {
                path.push_str(rest);
                self.bump_n(skip);
                self.newlines += 1;
            }
        }
        TokenKind::Path(path)
    }

    fn sees_number(&self) -> bool {
        let sign = usize::from(matches!(self.peek(), Some('+' | '-')));
        let digit = |n: usize| self.peek_at(n).is_some_and(|c| c.is_ascii_digit());
        digit(sign) || (self.peek_at(sign) == Some('.') && digit(sign + 1))
    }

    /// `[+-]? ([0-9]+ ('.' [0-9]*)? | '.' [0-9]+) ([eE] [+-]? [0-9]+)?`.
    fn number(&mut self, pos: Position) -> Result<TokenKind, ExpressError> {
        let start = self.at;
        if matches!(self.peek(), Some('+' | '-')) {
            self.bump();
        }
        self.eat_while(|c| c.is_ascii_digit());
        if self.peek() == Some('.') {
            self.bump();
            self.eat_while(|c| c.is_ascii_digit());
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            let sign = usize::from(matches!(self.peek_at(1), Some('+' | '-')));
            if self.peek_at(1 + sign).is_some_and(|c| c.is_ascii_digit()) {
                self.bump_n(1 + sign);
                self.eat_while(|c| c.is_ascii_digit());
            }
        }
        let text = &self.text[start..self.at];
        number_literal(text).map(TokenKind::Number).ok_or_else(|| {
            ExpressError::parse(pos, format!("number literal {text} is out of range"))
        })
    }

    /// A standard or triple-quoted string.
    fn string(&mut self, pos: Position) -> Result<TokenKind, ExpressError> {
        let rest = self.rest();
        if let Some(body) = rest.strip_prefix("\"\"\"") {
            if let Some(end) = escaped_end(body, "\"\"\"") {
                let inner = &body[..end];
                self.consume(&rest[..end + 6]);
                return Ok(TokenKind::Str(unescape(inner)));
            }
        }
        let Some(end) = escaped_end(&rest[1..], "\"") else {
            return Err(ExpressError::parse(pos, "unterminated string literal"));
        };
        let inner = &rest[1..=end];
        self.consume(&rest[..end + 2]);
        let inner = inner.trim_matches(['\r', '\n']);
        Ok(TokenKind::Str(unescape(inner)))
    }

    /// `r"..."` or `r"""..."""`, or `None` when the `r` starts an identifier
    /// instead: a single-quoted raw string must close on its own line.
    fn raw_string(&mut self, pos: Position) -> Result<Option<TokenKind>, ExpressError> {
        let rest = self.rest();
        if let Some(body) = rest[1..].strip_prefix("\"\"\"") {
            let Some(end) = body.find("\"\"\"") else {
                return Err(ExpressError::parse(pos, "unterminated raw string literal"));
            };
            self.consume(&rest[..end + 7]);
            return Ok(Some(TokenKind::Str(body[..end].to_owned())));
        }
        let body = &rest[2..];
        match body.find(['"', '\r', '\n']) {
            Some(end) if body[end..].starts_with('"') => {
                self.bump_n(body[..end].chars().count() + 3);
                Ok(Some(TokenKind::Str(body[..end].to_owned())))
            }
            _ => Ok(None),
        }
    }

    /// Consumes `consumed`, which may span lines, as one token.
    fn consume(&mut self, consumed: &str) {
        self.bump_n(consumed.chars().count());
    }
}

fn count_newlines(text: &str) -> u32 {
    u32::try_from(text.matches('\n').count()).unwrap_or(u32::MAX)
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '/'
}

fn is_ascii_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// When `rest`, the text after a path ending in `/`, is a line break and a
/// continuation of the path, the bytes to skip and the continuation. Not
/// when the next line assigns to an identifier.
fn split_path_continuation(rest: &str) -> Option<(usize, &str)> {
    let after_blanks = rest.trim_start_matches([' ', '\t']);
    let after_break = after_blanks
        .strip_prefix("\r\n")
        .or_else(|| after_blanks.strip_prefix('\n'))?;
    let next = after_break.trim_start_matches([' ', '\t']);
    let len = next.find(|c| !is_path_char(c)).unwrap_or(next.len());
    let continuation = &next[..len];
    if continuation.is_empty() {
        return None;
    }
    let follows = next[len..].trim_start_matches([' ', '\t']);
    let assigns = follows.starts_with('=') && !follows.starts_with("==");
    if is_ascii_identifier(continuation) && assigns {
        return None;
    }
    let skip = rest.len() - next.len() + len;
    Some((rest[..skip].chars().count(), continuation))
}

/// The byte offset in `body` of the first `close` not escaped by `\`.
fn escaped_end(body: &str, close: &str) -> Option<usize> {
    let mut chars = body.char_indices();
    while let Some((at, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if body[at..].starts_with(close) {
            return Some(at);
        }
    }
    None
}

/// Resolves `\n`, `\r`, `\t`, `\\` and `\"`. Any other escape stays as
/// written, backslash included.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') | None => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

/// The JSON number a `NUMBER` token denotes: an integer when it has no `.`
/// or exponent and fits, else a double. `None` when the double is not
/// finite.
fn number_literal(text: &str) -> Option<Number> {
    if !text.contains(['.', 'e', 'E']) {
        let unsigned = text.trim_start_matches('+');
        if let Ok(value) = unsigned.parse::<u64>() {
            return Some(Number::from(value));
        }
        if let Ok(value) = unsigned.parse::<i64>() {
            return Some(Number::from(value));
        }
    }
    let value: f64 = text.parse().ok()?;
    Number::from_f64(value)
}

/// The text the compiler reads when the block is wrapped in `<a2ui>` tags,
/// or `None` when it has none. Tags inside strings and comments do not
/// count. With an opening tag, only what lies between opening and closing
/// tags is kept; with only a closing tag, what lies before it. Dropped text
/// becomes spaces, line breaks kept, so positions still match the source.
fn strip_sentinels(source: &str) -> Option<String> {
    if !source.to_ascii_lowercase().contains("a2ui") {
        return None;
    }
    let segments = sentinel_segments(source);
    let has_open = segments
        .iter()
        .any(|(code, range)| *code && find_tag(&source[range.clone()], Tag::Open).is_some());
    if !has_open
        && !segments
            .iter()
            .any(|(code, range)| *code && find_tag(&source[range.clone()], Tag::Close).is_some())
    {
        return None;
    }
    let mut out = String::with_capacity(source.len());
    let mut inside = !has_open;
    let mut done = false;
    for (code, range) in segments {
        let text = &source[range];
        if done {
            out.push_str(&blank(text));
            continue;
        }
        if !code {
            out.push_str(&if inside { text.to_owned() } else { blank(text) });
            continue;
        }
        let mut rest = text;
        while !rest.is_empty() {
            let close = find_tag(rest, Tag::Close);
            let open = find_tag(rest, Tag::Open);
            if !inside {
                let Some((_, end)) = open else {
                    out.push_str(&blank(rest));
                    break;
                };
                out.push_str(&blank(&rest[..end]));
                inside = true;
                rest = &rest[end..];
                continue;
            }
            match (open, close) {
                (Some((start, end)), close) if close.is_none_or(|(c, _)| start < c) => {
                    out.push_str(&rest[..start]);
                    out.push_str(&blank(&rest[start..end]));
                    rest = &rest[end..];
                }
                (_, Some((start, end))) => {
                    out.push_str(&rest[..start]);
                    out.push_str(&blank(&rest[start..end]));
                    inside = false;
                    rest = &rest[end..];
                    if !has_open {
                        out.push_str(&blank(rest));
                        done = true;
                        break;
                    }
                }
                _ => {
                    out.push_str(rest);
                    break;
                }
            }
        }
    }
    Some(out)
}

#[derive(Clone, Copy)]
enum Tag {
    Open,
    Close,
}

/// The byte range of the first `<a2ui ...>` or `</a2ui>` tag in `text`,
/// matched without regard to case.
fn find_tag(text: &str, tag: Tag) -> Option<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let needle = match tag {
        Tag::Open => "<a2ui",
        Tag::Close => "</a2ui",
    };
    let mut from = 0;
    while let Some(found) = lower[from..].find(needle) {
        let start = from + found;
        let after = &lower[start + needle.len()..];
        let end = match tag {
            Tag::Open if after.starts_with('>') => Some(1),
            Tag::Open if after.starts_with(char::is_whitespace) => after.find('>').map(|i| i + 1),
            Tag::Open => None,
            Tag::Close => {
                let blanks = after.len() - after.trim_start().len();
                after[blanks..].starts_with('>').then_some(blanks + 1)
            }
        };
        if let Some(end) = end {
            return Some((start, start + needle.len() + end));
        }
        from = start + 1;
    }
    None
}

/// `text` with every character but a line break turned into a space.
fn blank(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' { '\n' } else { ' ' })
        .collect()
}

/// Splits `source` into code (`true`) and string or comment (`false`)
/// segments, by byte range. A line comment ends at a closing tag.
fn sentinel_segments(source: &str) -> Vec<(bool, std::ops::Range<usize>)> {
    let mut segments = Vec::new();
    let mut code_start = 0;
    let mut at = 0;
    while at < source.len() {
        let rest = &source[at..];
        let raw = rest.strip_prefix(['r', 'R']);
        let literal_len = if let Some(body) = raw.and_then(|r| r.strip_prefix("\"\"\"")) {
            Some(body.find("\"\"\"").map_or(rest.len(), |end| end + 7))
        } else if let Some(body) = raw.and_then(|r| r.strip_prefix('"')) {
            Some(body.find(['"', '\r', '\n']).map_or(rest.len(), |end| {
                if body[end..].starts_with('"') {
                    end + 3
                } else {
                    end + 2
                }
            }))
        } else if let Some(body) = rest.strip_prefix("\"\"\"") {
            Some(escaped_end(body, "\"\"\"").map_or(rest.len(), |end| end + 6))
        } else if let Some(body) = rest.strip_prefix('"') {
            let line_end = body.find(['\r', '\n']).unwrap_or(body.len());
            Some(escaped_end(&body[..line_end], "\"").map_or(line_end + 1, |end| end + 2))
        } else if rest.starts_with('#') || rest.starts_with("//") || rest.starts_with("--") {
            let line_end = rest.find('\n').unwrap_or(rest.len());
            Some(find_tag(&rest[..line_end], Tag::Close).map_or(line_end, |(start, _)| start))
        } else {
            rest.strip_prefix("/*")
                .map(|body| body.find("*/").map_or(rest.len(), |end| end + 4))
        };
        match literal_len {
            Some(len) if len > 0 => {
                if code_start < at {
                    segments.push((true, code_start..at));
                }
                segments.push((false, at..at + len));
                at += len;
                code_start = at;
            }
            _ => at += rest.chars().next().map_or(1, char::len_utf8),
        }
    }
    if code_start < source.len() {
        segments.push((true, code_start..source.len()));
    }
    segments
}

/// Joins identifiers broken across a line, as the compile suite tolerates:
/// a statement whose assigned name is split, and an argument whose name is
/// split when the joined name is assigned and neither half is.
fn join_split_identifiers(tokens: &mut Vec<Token>) {
    let depths = bracket_depths(tokens);
    let ident = |i: usize| match tokens.get(i).map(|t| &t.kind) {
        Some(TokenKind::Ident(name)) if is_ascii_identifier(name) => Some(name.as_str()),
        _ => None,
    };
    let line_start = |i: usize| i == 0 || tokens[i].newlines_before > 0;
    let one_break = |i: usize| tokens[i].newlines_before == 1 && tokens[i].clean_gap;
    let same_line = |i: usize, kind: &TokenKind| {
        tokens
            .get(i)
            .is_some_and(|t| t.newlines_before == 0 && t.kind == *kind)
    };
    let split_lhs = |i: usize| {
        let p1 = ident(i)?;
        let p2 = ident(i + 1)?;
        (line_start(i)
            && depths[i] == 0
            && one_break(i + 1)
            && same_line(i + 2, &TokenKind::Equals))
        .then_some((p1, p2))
    };
    let second_halves: HashSet<usize> = (0..tokens.len())
        .filter(|&i| split_lhs(i).is_some())
        .map(|i| i + 1)
        .collect();
    let direct: HashSet<&str> = (0..tokens.len())
        .filter(|&i| {
            line_start(i)
                && depths[i] == 0
                && !second_halves.contains(&i)
                && same_line(i + 1, &TokenKind::Equals)
        })
        .filter_map(ident)
        .collect();
    let mut declared: HashSet<String> = direct.iter().map(|&name| name.to_owned()).collect();
    let mut joins = Vec::new();
    for i in 0..tokens.len() {
        if let Some((p1, p2)) = split_lhs(i) {
            if !direct.contains(p1) && !direct.contains(p2) {
                declared.insert(format!("{p1}{p2}"));
                joins.push(i);
            }
        }
    }
    let opens_argument = |i: usize| {
        i > 0
            && matches!(
                tokens[i - 1].kind,
                TokenKind::LParen
                    | TokenKind::LBracket
                    | TokenKind::Comma
                    | TokenKind::Colon
                    | TokenKind::Equals
            )
            && tokens[i].newlines_before <= 1
            && tokens[i].clean_gap
    };
    let ends_argument = |i: usize| match tokens.get(i) {
        Some(t) if t.newlines_before == 0 => matches!(
            t.kind,
            TokenKind::Comma | TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace
        ),
        Some(t) => t.clean_gap,
        None => false,
    };
    for i in 0..tokens.len() {
        let (Some(p1), Some(p2)) = (ident(i), ident(i + 1)) else {
            continue;
        };
        if opens_argument(i)
            && one_break(i + 1)
            && ends_argument(i + 2)
            && declared.contains(&format!("{p1}{p2}"))
            && !declared.contains(p1)
            && !declared.contains(p2)
            && !joins.iter().any(|&j| j + 1 >= i && j <= i + 1)
        {
            joins.push(i);
        }
    }
    joins.sort_unstable();
    for &i in joins.iter().rev() {
        let second = tokens.remove(i + 1);
        if let (TokenKind::Ident(first), TokenKind::Ident(rest)) =
            (&mut tokens[i].kind, second.kind)
        {
            first.push_str(&rest);
        }
        if let Some(next) = tokens.get_mut(i + 1) {
            next.newlines_before += 1;
        }
    }
}

/// The bracket depth at each token, before it.
fn bracket_depths(tokens: &[Token]) -> Vec<u32> {
    let mut depth: u32 = 0;
    tokens
        .iter()
        .map(|token| {
            let before = depth;
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            before
        })
        .collect()
}

/// The JSON value of a literal token, or `None` for any other token.
pub(crate) fn literal_value(kind: &TokenKind) -> Option<Value> {
    match kind {
        TokenKind::Number(number) => Some(Value::Number(number.clone())),
        TokenKind::Str(text) => Some(Value::String(text.clone())),
        TokenKind::Bool(value) => Some(Value::Bool(*value)),
        TokenKind::Null => Some(Value::Null),
        _ => None,
    }
}
