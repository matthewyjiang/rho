//! The A2UI regex dialect: a portable subset of ECMAScript patterns in
//! Unicode mode, a static safety check, and its translation for matching.
//!
//! A pattern is parsed by the dialect's own grammar, so this renderer accepts
//! and rejects exactly the patterns the agent and other renderers do, then
//! checked against the four safety rules, and only then translated into the
//! `fancy-regex` syntax with every class spelled out, so no default of the
//! host engine leaks into what a pattern means.

use std::fmt::Write as _;

use fancy_regex::RegexBuilder;
use thiserror::Error;

/// The longest pattern, in code points, the safety check accepts.
const MAX_PATTERN_CHARS: usize = 256;

/// The longest value, in code points, a pattern is matched against. A longer
/// value does not match.
const MAX_VALUE_CHARS: usize = 4096;

/// The largest bound a `{m,n}` quantifier may give.
const MAX_REPEAT: u32 = 65_535;

/// Backtracking steps a match may take before it fails. Only patterns with
/// lookahead or `\b` backtrack; the safety check keeps them far below this.
const BACKTRACK_LIMIT: usize = 1_000_000;

const MAX_CODE_POINT: u32 = 0x10_FFFF;
const SURROGATES: (u32, u32) = (0xD800, 0xDFFF);

/// ECMAScript's word characters, which `\w` and `\b` use: ASCII only.
const WORD: &[(u32, u32)] = &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)];
const DIGIT: &[(u32, u32)] = &[(0x30, 0x39)];
/// ECMAScript's `WhiteSpace` and `LineTerminator` code points, which `\s` uses.
const SPACE: &[(u32, u32)] = &[
    (0x09, 0x0D),
    (0x20, 0x20),
    (0xA0, 0xA0),
    (0x1680, 0x1680),
    (0x2000, 0x200A),
    (0x2028, 0x2029),
    (0x202F, 0x202F),
    (0x205F, 0x205F),
    (0x3000, 0x3000),
    (0xFEFF, 0xFEFF),
];
/// What `.` matches: everything but the line terminators.
const DOT: &[(u32, u32)] = &[
    (0x00, 0x09),
    (0x0B, 0x0C),
    (0x0E, 0x2027),
    (0x202A, MAX_CODE_POINT),
];

/// Why a pattern was refused. The token at the start of each message is the
/// stable one the conformance suite and other renderers match on.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegexError {
    /// The pattern is longer than 256 code points (rule 1).
    #[error("PATTERN_TOO_LONG: pattern of {chars} code points exceeds {MAX_PATTERN_CHARS}")]
    PatternTooLong {
        /// The pattern's length in code points.
        chars: usize,
    },
    /// The pattern does not parse in the dialect, or uses a construct it
    /// leaves out.
    #[error("UNSUPPORTED_SYNTAX: {reason} at code point {offset}")]
    UnsupportedSyntax {
        /// What is wrong.
        reason: &'static str,
        /// Where, in code points from the start of the pattern.
        offset: usize,
    },
    /// A quantifier with an upper bound above 1 applies to a subexpression
    /// with an unbounded quantifier (rule 2).
    #[error("NESTED_QUANTIFIER: a repeated subexpression contains an unbounded quantifier")]
    NestedQuantifier,
    /// An unbounded quantifier applies to an alternation whose branches can
    /// start with the same character (rule 3).
    #[error(
        "OVERLAPPING_ALTERNATION: repeated alternation branches can start with the same character"
    )]
    OverlappingAlternation,
    /// Two unbounded quantified atoms with overlapping characters are
    /// adjacent but for nullable items (rule 4).
    #[error("ADJACENT_OVERLAP: adjacent unbounded quantifiers can match the same characters")]
    AdjacentOverlap,
    /// The pattern passed the dialect but is too large for the matcher.
    #[error("pattern is too large to compile")]
    TooLarge,
    /// A match took more backtracking steps than allowed.
    #[error("pattern match exceeded the backtracking limit")]
    BacktrackLimit,
}

impl RegexError {
    /// The dialect's diagnostic token, such as `UNSUPPORTED_SYNTAX`, or
    /// `None` for a failure of the matcher rather than of the pattern.
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::PatternTooLong { .. } => Some("PATTERN_TOO_LONG"),
            Self::UnsupportedSyntax { .. } => Some("UNSUPPORTED_SYNTAX"),
            Self::NestedQuantifier => Some("NESTED_QUANTIFIER"),
            Self::OverlappingAlternation => Some("OVERLAPPING_ALTERNATION"),
            Self::AdjacentOverlap => Some("ADJACENT_OVERLAP"),
            Self::TooLarge | Self::BacktrackLimit => None,
        }
    }
}

/// A pattern that passed the dialect and the safety check.
#[derive(Debug)]
pub(crate) struct Pattern(fancy_regex::Regex);

impl Pattern {
    /// Parses, checks and compiles `pattern`.
    pub(crate) fn new(pattern: &str) -> Result<Self, RegexError> {
        let chars: Vec<char> = pattern.chars().collect();
        if chars.len() > MAX_PATTERN_CHARS {
            return Err(RegexError::PatternTooLong { chars: chars.len() });
        }
        let tree = Parser { chars, pos: 0 }.pattern()?;
        check_safety(&tree)?;
        let mut source = String::new();
        render(&tree, &mut source);
        RegexBuilder::new(&source)
            .backtrack_limit(BACKTRACK_LIMIT)
            .build()
            .map(Self)
            .map_err(|_| RegexError::TooLarge)
    }

    /// Whether the pattern matches anywhere in `value`. A value longer than
    /// 4,096 code points does not match.
    pub(crate) fn is_match(&self, value: &str) -> Result<bool, RegexError> {
        if value.chars().count() > MAX_VALUE_CHARS {
            return Ok(false);
        }
        self.0
            .is_match(value)
            .map_err(|_| RegexError::BacktrackLimit)
    }
}

/// A set of code points: sorted, merged inclusive ranges with no surrogates.
type Set = Vec<(u32, u32)>;

#[derive(Debug, Clone, Copy)]
enum Anchor {
    Start,
    End,
    WordBoundary,
    NotWordBoundary,
}

/// A parsed pattern. Capturing and laziness are dropped: a match only has
/// to say whether it found one.
#[derive(Debug)]
enum Node {
    Empty,
    Set(Set),
    Anchor(Anchor),
    Group(Box<Node>),
    Lookahead {
        negative: bool,
        body: Box<Node>,
    },
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        body: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
}

/// One item inside a character class.
enum ClassAtom {
    Char(u32),
    Set(Set),
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn unsupported(&self, reason: &'static str) -> RegexError {
        RegexError::UnsupportedSyntax {
            reason,
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_nth(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    fn next(&mut self, reason: &'static str) -> Result<char, RegexError> {
        let c = self.peek().ok_or_else(|| self.unsupported(reason))?;
        self.pos += 1;
        Ok(c)
    }

    fn eat(&mut self, c: char) -> bool {
        let found = self.peek() == Some(c);
        if found {
            self.pos += 1;
        }
        found
    }

    fn pattern(mut self) -> Result<Node, RegexError> {
        let node = self.alternation()?;
        if self.peek().is_some() {
            return Err(self.unsupported("unmatched ')'"));
        }
        Ok(node)
    }

    fn alternation(&mut self) -> Result<Node, RegexError> {
        let mut branches = vec![self.concat()?];
        while self.eat('|') {
            branches.push(self.concat()?);
        }
        Ok(if branches.len() == 1 {
            branches.remove(0)
        } else {
            Node::Alt(branches)
        })
    }

    fn concat(&mut self) -> Result<Node, RegexError> {
        let mut items = Vec::new();
        while self.peek().is_some_and(|c| c != '|' && c != ')') {
            items.push(self.quantified()?);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.remove(0),
            _ => Node::Concat(items),
        })
    }

    fn quantified(&mut self) -> Result<Node, RegexError> {
        let atom = self.atom()?;
        if !matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
            return Ok(atom);
        }
        if matches!(atom, Node::Anchor(_) | Node::Lookahead { .. }) {
            return Err(self.unsupported("a zero-width assertion cannot be quantified"));
        }
        let (min, max) = self.bounds()?;
        self.eat('?');
        if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
            return Err(self.unsupported("possessive or chained quantifiers"));
        }
        Ok(Node::Repeat {
            body: Box::new(atom),
            min,
            max,
        })
    }

    fn bounds(&mut self) -> Result<(u32, Option<u32>), RegexError> {
        match self.next("missing quantifier")? {
            '*' => return Ok((0, None)),
            '+' => return Ok((1, None)),
            '?' => return Ok((0, Some(1))),
            _ => {}
        }
        let min = self
            .decimal()?
            .ok_or_else(|| self.unsupported("expected digits after '{'"))?;
        let max = if self.eat(',') {
            if self.peek() == Some('}') {
                None
            } else {
                Some(
                    self.decimal()?
                        .ok_or_else(|| self.unsupported("expected digits or '}' after ','"))?,
                )
            }
        } else {
            Some(min)
        };
        if !self.eat('}') {
            return Err(self.unsupported("unclosed '{m,n}' quantifier"));
        }
        if min > MAX_REPEAT || max.is_some_and(|max| max > MAX_REPEAT) {
            return Err(self.unsupported("quantifier bound above 65535"));
        }
        if max.is_some_and(|max| max < min) {
            return Err(self.unsupported("quantifier range out of order"));
        }
        Ok((min, max))
    }

    fn decimal(&mut self) -> Result<Option<u32>, RegexError> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(digit) = self.peek().and_then(|c| c.to_digit(10)) {
            self.pos += 1;
            value = value
                .checked_mul(10)
                .and_then(|v| v.checked_add(digit))
                .ok_or_else(|| self.unsupported("quantifier bound overflow"))?;
        }
        Ok((self.pos > start).then_some(value))
    }

    fn atom(&mut self) -> Result<Node, RegexError> {
        let c = self.next("unexpected end of pattern")?;
        Ok(match c {
            '^' => Node::Anchor(Anchor::Start),
            '$' => Node::Anchor(Anchor::End),
            '.' => Node::Set(DOT.to_vec()),
            '(' => self.group()?,
            '[' => self.class()?,
            '\\' => self.atom_escape()?,
            '*' | '+' | '?' | '{' => return Err(self.unsupported("quantifier without an atom")),
            '}' => return Err(self.unsupported("unescaped '}'")),
            ']' => return Err(self.unsupported("unescaped ']'")),
            ')' => return Err(self.unsupported("unmatched ')'")),
            literal => single(u32::from(literal)),
        })
    }

    fn group(&mut self) -> Result<Node, RegexError> {
        let lookahead = if self.eat('?') {
            match self.next("unsupported group construct")? {
                ':' => None,
                '=' => Some(false),
                '!' => Some(true),
                _ => return Err(self.unsupported("unsupported group construct")),
            }
        } else {
            None
        };
        let body = Box::new(self.alternation()?);
        if !self.eat(')') {
            return Err(self.unsupported("unclosed group"));
        }
        Ok(match lookahead {
            Some(negative) => Node::Lookahead { negative, body },
            None => Node::Group(body),
        })
    }

    fn atom_escape(&mut self) -> Result<Node, RegexError> {
        let c = self.next("trailing '\\'")?;
        if c == 'b' {
            return Ok(Node::Anchor(Anchor::WordBoundary));
        }
        if c == 'B' {
            return Ok(Node::Anchor(Anchor::NotWordBoundary));
        }
        if let Some(set) = class_escape(c) {
            return Ok(Node::Set(set));
        }
        match self.scalar_escape(c)? {
            Some(code) => Ok(single(code)),
            None => Err(self.unsupported("unsupported escape")),
        }
    }

    /// The code point an escape stands for, or `None` when `c` starts no
    /// scalar escape: control escapes, `\0`, escaped syntax characters,
    /// `\xHH`, `\uHHHH` and `\u{H...}`.
    fn scalar_escape(&mut self, c: char) -> Result<Option<u32>, RegexError> {
        Ok(Some(match c {
            't' => 0x09,
            'n' => 0x0A,
            'v' => 0x0B,
            'f' => 0x0C,
            'r' => 0x0D,
            '0' if self.peek().is_some_and(|c| c.is_ascii_digit()) => {
                return Err(self.unsupported("octal escapes"));
            }
            '0' => 0,
            '^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|'
            | '/' => u32::from(c),
            'x' => self.hex(2)?,
            'u' if self.eat('{') => {
                let mut code: u32 = 0;
                let mut digits = 0;
                while let Some(digit) = self.peek().and_then(|c| c.to_digit(16)) {
                    self.pos += 1;
                    digits += 1;
                    code = code * 16 + digit;
                    if code > MAX_CODE_POINT {
                        return Err(self.unsupported("code point above U+10FFFF"));
                    }
                }
                if digits == 0 || !self.eat('}') {
                    return Err(self.unsupported("malformed '\\u{...}' escape"));
                }
                self.scalar(code)?
            }
            'u' => {
                let code = self.hex(4)?;
                self.scalar(code)?
            }
            _ => return Ok(None),
        }))
    }

    fn hex(&mut self, digits: usize) -> Result<u32, RegexError> {
        let mut code = 0;
        for _ in 0..digits {
            let digit = self
                .peek()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| self.unsupported("malformed hex escape"))?;
            self.pos += 1;
            code = code * 16 + digit;
        }
        Ok(code)
    }

    fn scalar(&self, code: u32) -> Result<u32, RegexError> {
        if (SURROGATES.0..=SURROGATES.1).contains(&code) {
            return Err(self.unsupported("surrogate code point"));
        }
        Ok(code)
    }

    fn class(&mut self) -> Result<Node, RegexError> {
        let negated = self.eat('^');
        if self.peek() == Some(']') {
            return Err(self.unsupported("empty character class"));
        }
        let mut ranges = Vec::new();
        while self.peek().is_some_and(|c| c != ']') {
            if self.peek() == Some('[') && matches!(self.peek_nth(1), Some(':' | '.' | '=')) {
                return Err(self.unsupported("POSIX character classes"));
            }
            let left = self.class_atom()?;
            if self.peek() == Some('-') && self.peek_nth(1).is_some_and(|c| c != ']') {
                self.pos += 1;
                if self.peek() == Some('-') {
                    return Err(self.unsupported("character class subtraction"));
                }
                let right = self.class_atom()?;
                let (ClassAtom::Char(lo), ClassAtom::Char(hi)) = (left, right) else {
                    return Err(self.unsupported("a class escape as a range endpoint"));
                };
                if lo > hi {
                    return Err(self.unsupported("character class range out of order"));
                }
                ranges.push((lo, hi));
            } else {
                match left {
                    ClassAtom::Char(code) => ranges.push((code, code)),
                    ClassAtom::Set(set) => ranges.extend(set),
                }
            }
        }
        if !self.eat(']') {
            return Err(self.unsupported("unclosed character class"));
        }
        Ok(Node::Set(if negated {
            complement(&ranges)
        } else {
            normalize(ranges)
        }))
    }

    fn class_atom(&mut self) -> Result<ClassAtom, RegexError> {
        let c = self.next("unclosed character class")?;
        match c {
            '[' => Err(self.unsupported("unescaped '[' in a character class")),
            '&' if self.peek() == Some('&') => {
                Err(self.unsupported("character class intersection"))
            }
            '\\' => {
                let escaped = self.next("trailing '\\' in a character class")?;
                match escaped {
                    'b' => Ok(ClassAtom::Char(0x08)),
                    '-' => Ok(ClassAtom::Char(0x2D)),
                    _ => {
                        if let Some(set) = class_escape(escaped) {
                            return Ok(ClassAtom::Set(set));
                        }
                        match self.scalar_escape(escaped)? {
                            Some(code) => Ok(ClassAtom::Char(code)),
                            None => {
                                Err(self.unsupported("unsupported escape in a character class"))
                            }
                        }
                    }
                }
            }
            literal => Ok(ClassAtom::Char(u32::from(literal))),
        }
    }
}

fn single(code: u32) -> Node {
    Node::Set(vec![(code, code)])
}

fn class_escape(c: char) -> Option<Set> {
    Some(match c {
        'd' => DIGIT.to_vec(),
        'D' => complement(DIGIT),
        'w' => WORD.to_vec(),
        'W' => complement(WORD),
        's' => SPACE.to_vec(),
        'S' => complement(SPACE),
        _ => return None,
    })
}

/// Sorts and merges `ranges`, dropping surrogates.
fn normalize(ranges: Vec<(u32, u32)>) -> Set {
    let mut ranges: Vec<(u32, u32)> = ranges.into_iter().flat_map(without_surrogates).collect();
    ranges.sort_unstable();
    let mut merged: Set = Vec::with_capacity(ranges.len());
    for (lo, hi) in ranges {
        match merged.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => merged.push((lo, hi)),
        }
    }
    merged
}

fn without_surrogates((lo, hi): (u32, u32)) -> Vec<(u32, u32)> {
    let (s_lo, s_hi) = SURROGATES;
    if hi < s_lo || lo > s_hi {
        return vec![(lo, hi)];
    }
    let mut out = Vec::new();
    if lo < s_lo {
        out.push((lo, s_lo - 1));
    }
    if hi > s_hi {
        out.push((s_hi + 1, hi));
    }
    out
}

/// Every scalar value not in `ranges`.
fn complement(ranges: &[(u32, u32)]) -> Set {
    let mut out = Vec::new();
    let mut next = 0;
    for &(lo, hi) in &normalize(ranges.to_vec()) {
        if lo > next {
            out.push((next, lo - 1));
        }
        next = hi + 1;
    }
    if next <= MAX_CODE_POINT {
        out.push((next, MAX_CODE_POINT));
    }
    normalize(out)
}

fn union(a: &[(u32, u32)], b: &[(u32, u32)]) -> Set {
    normalize(a.iter().chain(b).copied().collect())
}

fn intersects(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    a.iter()
        .any(|&(a_lo, a_hi)| b.iter().any(|&(b_lo, b_hi)| a_lo <= b_hi && b_lo <= a_hi))
}

// The safety check, rules 2 to 4. Rule 1, the length, is checked before
// parsing.

fn check_safety(tree: &Node) -> Result<(), RegexError> {
    if has_nested_quantifier(tree) {
        return Err(RegexError::NestedQuantifier);
    }
    if has_overlapping_alternation(tree) {
        return Err(RegexError::OverlappingAlternation);
    }
    if has_adjacent_overlap(tree) {
        return Err(RegexError::AdjacentOverlap);
    }
    Ok(())
}

/// The children a rule recurses into. A lookahead counts: it backtracks too.
fn children(node: &Node) -> &[Node] {
    match node {
        Node::Empty | Node::Set(_) | Node::Anchor(_) => &[],
        Node::Group(body) | Node::Lookahead { body, .. } | Node::Repeat { body, .. } => {
            std::slice::from_ref(&**body)
        }
        Node::Concat(items) | Node::Alt(items) => items,
    }
}

/// A group holding one node counts as that node.
fn unwrap_group(mut node: &Node) -> &Node {
    while let Node::Group(body) = node {
        node = body;
    }
    node
}

/// Whether `node` can match the empty string.
fn nullable(node: &Node) -> bool {
    match node {
        Node::Empty | Node::Anchor(_) | Node::Lookahead { .. } => true,
        Node::Set(_) => false,
        Node::Group(body) => nullable(body),
        Node::Concat(items) => items.iter().all(nullable),
        Node::Alt(branches) => branches.iter().any(nullable),
        Node::Repeat { body, min, .. } => *min == 0 || nullable(body),
    }
}

/// The characters that can start a non-empty match of `node`.
fn first_set(node: &Node) -> Set {
    match node {
        Node::Empty
        | Node::Anchor(_)
        | Node::Lookahead { .. }
        | Node::Repeat { max: Some(0), .. } => Vec::new(),
        Node::Set(set) => set.clone(),
        Node::Group(body) | Node::Repeat { body, .. } => first_set(body),
        Node::Alt(branches) => branches
            .iter()
            .fold(Vec::new(), |acc, b| union(&acc, &first_set(b))),
        Node::Concat(items) => {
            let mut acc = Vec::new();
            for item in items {
                acc = union(&acc, &first_set(item));
                if !nullable(item) {
                    break;
                }
            }
            acc
        }
    }
}

/// Every character `node` can consume, outside lookaheads.
fn chars_set(node: &Node) -> Set {
    match node {
        Node::Empty
        | Node::Anchor(_)
        | Node::Lookahead { .. }
        | Node::Repeat { max: Some(0), .. } => Vec::new(),
        Node::Set(set) => set.clone(),
        other => children(other)
            .iter()
            .fold(Vec::new(), |acc, child| union(&acc, &chars_set(child))),
    }
}

fn has_unbounded(node: &Node) -> bool {
    matches!(node, Node::Repeat { max: None, .. }) || children(node).iter().any(has_unbounded)
}

/// Rule 2: a quantifier with an upper bound above 1 over an unbounded one.
fn has_nested_quantifier(node: &Node) -> bool {
    let here = match node {
        Node::Repeat { body, max, .. } => max.is_none_or(|max| max > 1) && has_unbounded(body),
        _ => false,
    };
    here || children(node).iter().any(has_nested_quantifier)
}

/// Rule 3: an unbounded quantifier over branches with a common first
/// character.
fn has_overlapping_alternation(node: &Node) -> bool {
    let here = match node {
        Node::Repeat {
            body, max: None, ..
        } => match unwrap_group(body) {
            Node::Alt(branches) => {
                let firsts: Vec<Set> = branches
                    .iter()
                    .map(|b| first_set(unwrap_group(b)))
                    .collect();
                firsts
                    .iter()
                    .enumerate()
                    .any(|(i, a)| firsts[i + 1..].iter().any(|b| intersects(a, b)))
            }
            _ => false,
        },
        _ => false,
    };
    here || children(node).iter().any(has_overlapping_alternation)
}

/// Rule 4: in one concatenation, two unbounded quantified atoms with common
/// characters and only nullable items between them.
fn has_adjacent_overlap(node: &Node) -> bool {
    let here = match node {
        Node::Concat(items) => items.iter().enumerate().any(|(i, item)| {
            let Some(left) = unbounded_body(item) else {
                return false;
            };
            let left = chars_set(left);
            for later in &items[i + 1..] {
                if unbounded_body(later).is_some_and(|right| intersects(&left, &chars_set(right))) {
                    return true;
                }
                if !nullable(later) {
                    return false;
                }
            }
            false
        }),
        _ => false,
    };
    here || children(node).iter().any(has_adjacent_overlap)
}

fn unbounded_body(node: &Node) -> Option<&Node> {
    match unwrap_group(node) {
        Node::Repeat {
            body, max: None, ..
        } => Some(body),
        _ => None,
    }
}

/// ASCII word characters as a class, for `\b` and `\B`.
const WORD_CLASS: &str = "[0-9A-Za-z_]";

fn render(node: &Node, out: &mut String) {
    match node {
        Node::Empty => {}
        Node::Set(set) => render_set(set, out),
        Node::Anchor(Anchor::Start) => out.push_str(r"\A"),
        Node::Anchor(Anchor::End) => out.push_str(r"\z"),
        Node::Anchor(Anchor::WordBoundary) => {
            let w = WORD_CLASS;
            let _ = write!(out, "(?:(?<={w})(?!{w})|(?<!{w})(?={w}))");
        }
        Node::Anchor(Anchor::NotWordBoundary) => {
            let w = WORD_CLASS;
            let _ = write!(out, "(?:(?<={w})(?={w})|(?<!{w})(?!{w}))");
        }
        Node::Group(body) => {
            out.push_str("(?:");
            render(body, out);
            out.push(')');
        }
        Node::Lookahead { negative, body } => {
            out.push_str(if *negative { "(?!" } else { "(?=" });
            render(body, out);
            out.push(')');
        }
        Node::Concat(items) => {
            for item in items {
                render(item, out);
            }
        }
        Node::Alt(branches) => {
            out.push_str("(?:");
            for (i, branch) in branches.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                render(branch, out);
            }
            out.push(')');
        }
        Node::Repeat { body, min, max } => {
            out.push_str("(?:");
            render(body, out);
            out.push(')');
            let _ = match max {
                Some(max) => write!(out, "{{{min},{max}}}"),
                None => write!(out, "{{{min},}}"),
            };
        }
    }
}

fn render_set(set: &[(u32, u32)], out: &mut String) {
    if set.is_empty() {
        // A class that matches nothing.
        let _ = write!(out, r"[^\x{{0}}-\x{{{MAX_CODE_POINT:X}}}]");
        return;
    }
    out.push('[');
    for &(lo, hi) in set {
        let _ = if lo == hi {
            write!(out, r"\x{{{lo:X}}}")
        } else {
            write!(out, r"\x{{{lo:X}}}-\x{{{hi:X}}}")
        };
    }
    out.push(']');
}

#[cfg(test)]
#[path = "regex_dialect_tests.rs"]
mod tests;
