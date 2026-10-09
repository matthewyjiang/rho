//! Pure text navigation for vim mode: lines, words, motions, and the ranges
//! operators cover. Everything works on the composer text as chars.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Operator {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WordKind {
    /// Letters, digits, and `_`, or a run of other punctuation.
    Word,
    /// Any run of non-blank characters.
    BigWord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FindKind {
    /// `f`: onto the next match.
    Forward,
    /// `F`: onto the previous match.
    Backward,
    /// `t`: just before the next match.
    TillForward,
    /// `T`: just after the previous match.
    TillBackward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ObjectScope {
    /// `iw`: the word (or blank run) alone.
    Inner,
    /// `aw`: the word plus surrounding blanks.
    Around,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Motion {
    Left,
    Right,
    LineStart,
    FirstNonBlank,
    LineEnd,
    WordForward(WordKind),
    WordBackward(WordKind),
    WordEnd(WordKind),
    LineDown,
    LineUp,
    /// `gg`, or line N with a count.
    FirstLine,
    /// `G`, or line N with a count.
    LastLine,
    Find(FindKind, char),
}

impl Motion {
    /// The motion a single normal-mode key names, if any.
    pub(super) fn from_char(ch: char) -> Option<Self> {
        let motion = match ch {
            'h' => Self::Left,
            'l' | ' ' => Self::Right,
            '0' => Self::LineStart,
            '^' => Self::FirstNonBlank,
            '$' => Self::LineEnd,
            'w' => Self::WordForward(WordKind::Word),
            'W' => Self::WordForward(WordKind::BigWord),
            'b' => Self::WordBackward(WordKind::Word),
            'B' => Self::WordBackward(WordKind::BigWord),
            'e' => Self::WordEnd(WordKind::Word),
            'E' => Self::WordEnd(WordKind::BigWord),
            'j' => Self::LineDown,
            'k' => Self::LineUp,
            'G' => Self::LastLine,
            _ => return None,
        };
        Some(motion)
    }
}

/// How an operator treats the span between the caret and a motion target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Span {
    /// The target character is not included.
    Exclusive,
    /// The target character is included.
    Inclusive,
    /// Whole lines from the caret's line through the target's line.
    Linewise,
}

/// What an operator acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Chars(Range<usize>),
    /// Line indexes `first..=last`.
    Lines {
        first: usize,
        last: usize,
    },
}

pub(super) fn line_start(chars: &[char], index: usize) -> usize {
    chars[..index.min(chars.len())]
        .iter()
        .rposition(|&ch| ch == '\n')
        .map_or(0, |newline| newline + 1)
}

/// Start and end (the newline or text end) of the line holding `index`.
pub(super) fn line_bounds(chars: &[char], index: usize) -> (usize, usize) {
    let index = index.min(chars.len());
    let end = chars[index..]
        .iter()
        .position(|&ch| ch == '\n')
        .map_or(chars.len(), |offset| index + offset);
    (line_start(chars, index), end)
}

pub(super) fn first_non_blank(chars: &[char], index: usize) -> usize {
    let (start, end) = line_bounds(chars, index);
    chars[start..end]
        .iter()
        .position(|ch| !ch.is_whitespace())
        .map_or(end, |offset| start + offset)
}

pub(super) fn line_starts(chars: &[char]) -> Vec<usize> {
    std::iter::once(0)
        .chain(
            chars
                .iter()
                .enumerate()
                .filter(|(_, &ch)| ch == '\n')
                .map(|(index, _)| index + 1),
        )
        .collect()
}

pub(super) fn line_index(chars: &[char], index: usize) -> usize {
    chars[..index.min(chars.len())]
        .iter()
        .filter(|&&ch| ch == '\n')
        .count()
}

/// Chars of lines `first..=last` an operator removes or replaces. Delete
/// and yank take a line break too; change keeps one line to type into.
pub(super) fn line_range(
    chars: &[char],
    operator: Operator,
    first: usize,
    last: usize,
) -> Range<usize> {
    let (start, end) = line_text_range(chars, first, last);
    match operator {
        Operator::Change => start..end,
        Operator::Delete | Operator::Yank if end < chars.len() => start..end + 1,
        Operator::Delete | Operator::Yank => start.saturating_sub(usize::from(start > 0))..end,
    }
}

/// Text of lines `first..=last` without the final line break.
pub(super) fn line_text_range(chars: &[char], first: usize, last: usize) -> (usize, usize) {
    let starts = line_starts(chars);
    let (_, end) = line_bounds(chars, starts[last]);
    (starts[first], end)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CharClass {
    Blank,
    Word,
    Punct,
}

fn class(ch: char, kind: WordKind) -> CharClass {
    if ch.is_whitespace() {
        CharClass::Blank
    } else if kind == WordKind::BigWord || ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

fn next_word_start(chars: &[char], index: usize, kind: WordKind) -> usize {
    let mut index = index;
    if let Some(&ch) = chars.get(index) {
        let start_class = class(ch, kind);
        if start_class != CharClass::Blank {
            while chars
                .get(index)
                .is_some_and(|&ch| class(ch, kind) == start_class)
            {
                index += 1;
            }
        }
    }
    while chars
        .get(index)
        .is_some_and(|&ch| class(ch, kind) == CharClass::Blank)
    {
        index += 1;
    }
    index
}

fn previous_word_start(chars: &[char], index: usize, kind: WordKind) -> usize {
    let mut index = index.min(chars.len());
    while index > 0 && class(chars[index - 1], kind) == CharClass::Blank {
        index -= 1;
    }
    let Some(&ch) = index.checked_sub(1).and_then(|before| chars.get(before)) else {
        return 0;
    };
    let word_class = class(ch, kind);
    while index > 0 && class(chars[index - 1], kind) == word_class {
        index -= 1;
    }
    index
}

/// Index of the last char of the next word end after `index`, or `index`
/// when no word follows.
fn word_end(chars: &[char], index: usize, kind: WordKind) -> usize {
    let start = index;
    let mut index = index + 1;
    while chars
        .get(index)
        .is_some_and(|&ch| class(ch, kind) == CharClass::Blank)
    {
        index += 1;
    }
    let Some(&ch) = chars.get(index) else {
        return start;
    };
    let word_class = class(ch, kind);
    while chars
        .get(index + 1)
        .is_some_and(|&ch| class(ch, kind) == word_class)
    {
        index += 1;
    }
    index
}

/// End (exclusive) of the same-class run starting at `index`, within its line.
fn word_run_end(chars: &[char], index: usize, kind: WordKind) -> usize {
    let Some(&ch) = chars.get(index) else {
        return index;
    };
    let run_class = class(ch, kind);
    let mut end = index;
    while chars
        .get(end)
        .is_some_and(|&ch| ch != '\n' && class(ch, kind) == run_class)
    {
        end += 1;
    }
    end
}

/// Repeat `step` up to `count` times, stopping once it no longer moves, so
/// a huge count costs no more than walking the text.
fn repeat(start: usize, count: usize, step: impl Fn(usize) -> usize) -> usize {
    let mut position = start;
    for _ in 0..count {
        let next = step(position);
        if next == position {
            break;
        }
        position = next;
    }
    position
}

/// Where `motion` lands from `cursor`. `line` is the explicit line number
/// for `gg` and `G`. `None` when the motion fails, like `f` with no match.
pub(super) fn motion_target(
    chars: &[char],
    cursor: usize,
    motion: Motion,
    count: usize,
    line: Option<usize>,
) -> Option<usize> {
    motion_span(chars, cursor, motion, count, line).map(|(target, _)| target)
}

fn motion_span(
    chars: &[char],
    cursor: usize,
    motion: Motion,
    count: usize,
    line: Option<usize>,
) -> Option<(usize, Span)> {
    let (start, end) = line_bounds(chars, cursor);
    let starts = line_starts(chars);
    let last_line = starts.len() - 1;
    let line_target = |line: usize| starts[line.min(last_line)];
    let current_line = line_index(chars, cursor);
    let target = match motion {
        Motion::Left => (cursor.saturating_sub(count).max(start), Span::Exclusive),
        Motion::Right => (cursor.saturating_add(count).min(end), Span::Exclusive),
        Motion::LineStart => (start, Span::Exclusive),
        Motion::FirstNonBlank => (first_non_blank(chars, cursor), Span::Exclusive),
        Motion::LineEnd => (end, Span::Exclusive),
        Motion::WordForward(kind) => (
            repeat(cursor, count, |at| next_word_start(chars, at, kind)),
            Span::Exclusive,
        ),
        Motion::WordBackward(kind) => (
            repeat(cursor, count, |at| previous_word_start(chars, at, kind)),
            Span::Exclusive,
        ),
        Motion::WordEnd(kind) => (
            repeat(cursor, count, |at| word_end(chars, at, kind)),
            Span::Inclusive,
        ),
        Motion::LineDown => (
            line_target(current_line.saturating_add(count)),
            Span::Linewise,
        ),
        Motion::LineUp => (
            line_target(current_line.saturating_sub(count)),
            Span::Linewise,
        ),
        Motion::FirstLine => (
            line_target(line.unwrap_or(1).saturating_sub(1)),
            Span::Linewise,
        ),
        Motion::LastLine => (
            line_target(line.map_or(last_line, |line| line.saturating_sub(1))),
            Span::Linewise,
        ),
        Motion::Find(kind, wanted) => {
            let found = match kind {
                FindKind::Forward | FindKind::TillForward => (cursor + 1..end)
                    .filter(|&index| chars[index] == wanted)
                    .nth(count - 1),
                FindKind::Backward | FindKind::TillBackward => (start..cursor)
                    .rev()
                    .filter(|&index| chars[index] == wanted)
                    .nth(count - 1),
            }?;
            match kind {
                FindKind::Forward => (found, Span::Inclusive),
                FindKind::TillForward => (found - 1, Span::Inclusive),
                FindKind::Backward => (found, Span::Exclusive),
                FindKind::TillBackward => (found + 1, Span::Exclusive),
            }
        }
    };
    Some(target)
}

/// What `operator` covers for `motion` from `cursor`.
pub(super) fn operator_target(
    chars: &[char],
    cursor: usize,
    operator: Operator,
    motion: Motion,
    count: usize,
    line: Option<usize>,
) -> Option<Target> {
    if let Motion::WordForward(kind) = motion {
        let on_word = chars
            .get(cursor)
            .is_some_and(|&ch| class(ch, kind) != CharClass::Blank);
        // `cw` on a word changes to the word's end, like `ce`, and keeps the
        // blank after it.
        if operator == Operator::Change && on_word {
            let end = repeat(cursor, count, |at| {
                if at == cursor {
                    word_run_end(chars, at, kind)
                } else {
                    word_run_end(chars, next_word_start(chars, at, kind), kind)
                }
            });
            return Some(Target::Chars(cursor..end));
        }
        // `dw` whose last word ends its line stops at the line break.
        let last_word = repeat(cursor, count - 1, |at| next_word_start(chars, at, kind));
        let target = next_word_start(chars, last_word, kind);
        let end = chars[last_word..target]
            .iter()
            .position(|&ch| ch == '\n')
            .filter(|&offset| offset > 0)
            .map_or(target, |offset| last_word + offset);
        return Some(Target::Chars(cursor..end));
    }
    let (target, span) = motion_span(chars, cursor, motion, count, line)?;
    let target = match span {
        Span::Exclusive => Target::Chars(cursor.min(target)..cursor.max(target)),
        Span::Inclusive => {
            Target::Chars(cursor.min(target)..(cursor.max(target) + 1).min(chars.len()))
        }
        Span::Linewise => Target::Lines {
            first: line_index(chars, cursor.min(target)),
            last: line_index(chars, cursor.max(target)),
        },
    };
    Some(target)
}

/// `iw` / `aw` around `cursor`, within its line.
pub(super) fn word_object(
    chars: &[char],
    cursor: usize,
    scope: ObjectScope,
    kind: WordKind,
) -> Option<Range<usize>> {
    let &ch = chars.get(cursor).filter(|&&ch| ch != '\n')?;
    let object_class = class(ch, kind);
    let mut start = cursor;
    while start > 0 && chars[start - 1] != '\n' && class(chars[start - 1], kind) == object_class {
        start -= 1;
    }
    let mut end = word_run_end(chars, cursor, kind);
    if scope == ObjectScope::Inner {
        return Some(start..end);
    }
    if object_class == CharClass::Blank {
        // `aw` on blanks takes the word after them.
        return Some(start..word_run_end(chars, end, kind));
    }
    let blank = |index: usize| chars[index] != '\n' && chars[index].is_whitespace();
    let word_end = end;
    while end < chars.len() && blank(end) {
        end += 1;
    }
    if end == word_end {
        while start > 0 && blank(start - 1) {
            start -= 1;
        }
    }
    Some(start..end)
}
