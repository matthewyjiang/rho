//! Vim-style modal editing for the main composer (`editing_mode = "vim"`).
//!
//! [`VimState`] interprets keys against a [`ComposerBuffer`]. Insert mode
//! leaves typing to the composer's shared edit path and only claims `Esc`.
//! Normal mode runs counts, motions, operators, and text objects. The owner
//! applies palette and history side effects from the returned [`VimOutcome`].
//!
//! The caret is a char index. Normal commands and painting project it onto
//! a character, including after edits made outside vim.

mod motion;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use self::motion::{
    first_non_blank, line_bounds, line_index, line_range, line_start, line_starts, line_text_range,
    motion_target, operator_target, word_object, FindKind, Motion, ObjectScope, Operator, Target,
    WordKind,
};
use super::composer_buffer::{ComposerBuffer, ComposerEditKey, Fragment};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum VimMode {
    /// Keys type text, as in the default editing mode.
    #[default]
    Insert,
    /// Keys run motions, operators, and commands.
    Normal,
}

impl VimMode {
    /// Composer chrome label for the mode indicator.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Insert => "INSERT",
            Self::Normal => "NORMAL",
        }
    }
}

/// Whether a consumed key changed the composer text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextChange {
    Edited,
    Unchanged,
}

/// What the owner does after [`VimState::handle_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VimOutcome {
    /// Not a vim key in this mode; handle it as usual.
    Unhandled,
    /// Consumed; run text-change bookkeeping on [`TextChange::Edited`].
    Handled(TextChange),
    /// Apply this shared edit key through the owner's path, which may recall
    /// prompt history.
    Forward(ComposerEditKey),
}

/// The key a multi-key command still waits for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Awaiting {
    #[default]
    Command,
    /// `g` waits for a second `g`.
    G,
    /// `r` waits for the replacement character.
    Replace,
    Find(FindKind),
    /// An operator's `i` or `a` waits for the object (`w` or `W`).
    TextObject(ObjectScope),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pending {
    count: Option<usize>,
    operator: Option<Operator>,
    /// Count typed before the operator; multiplies the motion count.
    operator_count: Option<usize>,
    awaiting: Awaiting,
}

impl Pending {
    fn total_count(self) -> usize {
        self.count
            .unwrap_or(1)
            .saturating_mul(self.operator_count.unwrap_or(1))
    }

    /// The typed count, if any, for motions where a count means a line number.
    fn explicit_count(self) -> Option<usize> {
        (self.count.is_some() || self.operator_count.is_some()).then(|| self.total_count())
    }
}

/// Text from the last delete, change, or yank, paste markers included.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Register {
    fragment: Fragment,
    linewise: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Placement {
    Before,
    After,
}

/// Preserve the distinction between typed count digits and special-key motions.
enum NormalInput {
    Char(char),
    Motion(Motion),
    Command(char),
}

/// Vim mode, any half-typed command, and the unnamed register.
#[derive(Clone, Debug, Default)]
pub(super) struct VimState {
    mode: VimMode,
    pending: Pending,
    register: Option<Register>,
}

impl VimState {
    pub(super) fn mode(&self) -> VimMode {
        self.mode
    }

    /// Whether `Esc` belongs to vim (leave insert mode or drop a half-typed
    /// command) rather than the composer's usual cancel or abort.
    pub(super) fn captures_esc(&self) -> bool {
        self.mode == VimMode::Insert || self.pending != Pending::default()
    }

    /// Start a fresh draft in insert mode, keeping the register.
    pub(super) fn reset(&mut self) {
        self.mode = VimMode::Insert;
        self.pending = Pending::default();
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent, buffer: &mut ComposerBuffer) -> VimOutcome {
        match self.mode {
            VimMode::Insert if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE => {
                self.enter_normal(buffer);
                VimOutcome::Handled(TextChange::Unchanged)
            }
            VimMode::Insert => VimOutcome::Unhandled,
            VimMode::Normal => self.normal_key(key, buffer),
        }
    }

    fn enter_normal(&mut self, buffer: &mut ComposerBuffer) {
        buffer.end_undo_group();
        self.mode = VimMode::Normal;
        self.pending = Pending::default();
        let chars = chars(buffer);
        let cursor = buffer.cursor().min(chars.len());
        // Like vim, leaving insert mode steps back onto the last typed char.
        if cursor > line_start(&chars, cursor) {
            buffer.set_cursor(buffer.caret_index(cursor - 1));
        }
        clamp_to_char(buffer);
    }

    fn enter_insert(&mut self) {
        self.mode = VimMode::Insert;
    }

    fn normal_key(&mut self, key: KeyEvent, buffer: &mut ComposerBuffer) -> VimOutcome {
        let pending = std::mem::take(&mut self.pending);
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return VimOutcome::Unhandled;
        }
        let input = match key.code {
            KeyCode::Char(ch) => NormalInput::Char(ch),
            KeyCode::Esc if pending != Pending::default() => {
                return VimOutcome::Handled(TextChange::Unchanged);
            }
            KeyCode::Left | KeyCode::Backspace => NormalInput::Motion(Motion::Left),
            KeyCode::Right => NormalInput::Motion(Motion::Right),
            KeyCode::Up => NormalInput::Motion(Motion::LineUp),
            KeyCode::Down => NormalInput::Motion(Motion::LineDown),
            KeyCode::End => NormalInput::Motion(Motion::LineEnd),
            KeyCode::Delete => NormalInput::Command('x'),
            KeyCode::Home => NormalInput::Motion(Motion::LineStart),
            _ => return VimOutcome::Unhandled,
        };
        buffer.clear_selection();
        clamp_to_char(buffer);
        // Each normal-mode command is its own undo step, so `xx` undoes one
        // `x` at a time. A command that enters insert mode keeps its group
        // open until Esc.
        buffer.begin_undo_group();
        let outcome = match input {
            NormalInput::Motion(motion) => self.motion(pending, motion, buffer),
            NormalInput::Char(ch) | NormalInput::Command(ch) => {
                self.normal_char(ch, pending, buffer)
            }
        };
        if self.mode == VimMode::Normal {
            buffer.end_undo_group();
        }
        outcome
    }

    fn normal_char(
        &mut self,
        ch: char,
        mut pending: Pending,
        buffer: &mut ComposerBuffer,
    ) -> VimOutcome {
        match pending.awaiting {
            Awaiting::Command => {}
            Awaiting::G if ch == 'g' => return self.motion(pending, Motion::FirstLine, buffer),
            Awaiting::Replace => return replace_chars(buffer, ch, pending.total_count()),
            Awaiting::Find(kind) => return self.motion(pending, Motion::Find(kind, ch), buffer),
            Awaiting::TextObject(scope) => {
                let kind = match ch {
                    'w' => WordKind::Word,
                    'W' => WordKind::BigWord,
                    _ => return VimOutcome::Handled(TextChange::Unchanged),
                };
                let chars = chars(buffer);
                return match (
                    pending.operator,
                    word_object(&chars, buffer.cursor(), scope, kind),
                ) {
                    (Some(operator), Some(range)) => {
                        self.operate(operator, Target::Chars(range), buffer)
                    }
                    _ => VimOutcome::Handled(TextChange::Unchanged),
                };
            }
            Awaiting::G => return VimOutcome::Handled(TextChange::Unchanged),
        }

        if let Some(digit) = ch.to_digit(10) {
            if digit != 0 || pending.count.is_some() {
                let count = pending.count.unwrap_or(0);
                pending.count = Some(count.saturating_mul(10).saturating_add(digit as usize));
                self.pending = pending;
                return VimOutcome::Handled(TextChange::Unchanged);
            }
        }

        let awaiting = match ch {
            'g' => Some(Awaiting::G),
            'f' => Some(Awaiting::Find(FindKind::Forward)),
            'F' => Some(Awaiting::Find(FindKind::Backward)),
            't' => Some(Awaiting::Find(FindKind::TillForward)),
            'T' => Some(Awaiting::Find(FindKind::TillBackward)),
            'i' if pending.operator.is_some() => Some(Awaiting::TextObject(ObjectScope::Inner)),
            'a' if pending.operator.is_some() => Some(Awaiting::TextObject(ObjectScope::Around)),
            'r' if pending.operator.is_none() => Some(Awaiting::Replace),
            _ => None,
        };
        if let Some(awaiting) = awaiting {
            pending.awaiting = awaiting;
            self.pending = pending;
            return VimOutcome::Handled(TextChange::Unchanged);
        }

        if let Some(motion) = Motion::from_char(ch) {
            return self.motion(pending, motion, buffer);
        }

        if let Some(operator) = pending.operator {
            // `dd`, `cc`, `yy`: the doubled operator acts on whole lines.
            return if operator_char(operator) == ch {
                self.operate_lines(operator, pending.total_count(), buffer)
            } else {
                VimOutcome::Handled(TextChange::Unchanged)
            };
        }

        let count = pending.count.unwrap_or(1);
        match ch {
            'd' | 'c' | 'y' => {
                let operator = match ch {
                    'd' => Operator::Delete,
                    'c' => Operator::Change,
                    _ => Operator::Yank,
                };
                self.pending = Pending {
                    operator: Some(operator),
                    operator_count: pending.count,
                    ..Pending::default()
                };
                VimOutcome::Handled(TextChange::Unchanged)
            }
            'x' => self.operate_motion(Operator::Delete, Motion::Right, count, buffer),
            'X' => self.operate_motion(Operator::Delete, Motion::Left, count, buffer),
            's' => self.operate_motion(Operator::Change, Motion::Right, count, buffer),
            'D' => self.operate_motion(Operator::Delete, Motion::LineEnd, 1, buffer),
            'C' => self.operate_motion(Operator::Change, Motion::LineEnd, 1, buffer),
            'S' => self.operate_lines(Operator::Change, count, buffer),
            'Y' => self.operate_lines(Operator::Yank, count, buffer),
            'p' => self.put(Placement::After, buffer),
            'P' => self.put(Placement::Before, buffer),
            'u' => {
                let mut change = TextChange::Unchanged;
                for _ in 0..count {
                    if !buffer.undo() {
                        break;
                    }
                    change = TextChange::Edited;
                }
                clamp_to_char(buffer);
                VimOutcome::Handled(change)
            }
            'i' | 'a' | 'I' | 'A' => {
                let chars = chars(buffer);
                let cursor = buffer.cursor();
                let (start, end) = line_bounds(&chars, cursor);
                let target = match ch {
                    'a' if end > start => buffer.caret_index_after((cursor + 1).min(end)),
                    'I' => first_non_blank(&chars, cursor),
                    'A' => end,
                    _ => cursor,
                };
                buffer.set_cursor(buffer.caret_index(target));
                self.enter_insert();
                VimOutcome::Handled(TextChange::Unchanged)
            }
            'o' | 'O' => {
                let chars = chars(buffer);
                let (start, end) = line_bounds(&chars, buffer.cursor());
                self.enter_insert();
                if ch == 'o' {
                    buffer.replace_range(end..end, Fragment::plain("\n"));
                } else {
                    buffer.replace_range(start..start, Fragment::plain("\n"));
                    buffer.set_cursor(start);
                }
                VimOutcome::Handled(TextChange::Edited)
            }
            _ => VimOutcome::Handled(TextChange::Unchanged),
        }
    }

    /// Move by `motion`, or apply the pending operator over it.
    fn motion(
        &mut self,
        pending: Pending,
        motion: Motion,
        buffer: &mut ComposerBuffer,
    ) -> VimOutcome {
        let count = pending.total_count();
        let chars = chars(buffer);
        let cursor = buffer.cursor();
        if let Some(operator) = pending.operator {
            return match operator_target(
                &chars,
                cursor,
                operator,
                motion,
                count,
                pending.explicit_count(),
            ) {
                Some(target) => self.operate(operator, target, buffer),
                None => VimOutcome::Handled(TextChange::Unchanged),
            };
        }
        let target = match motion {
            // A single j/k moves by painted row and recalls history at the
            // edges, like Down and Up.
            Motion::LineDown if count == 1 => return VimOutcome::Forward(ComposerEditKey::Down),
            Motion::LineUp if count == 1 => return VimOutcome::Forward(ComposerEditKey::Up),
            Motion::LineDown | Motion::LineUp => {
                let line = motion_target(&chars, cursor, motion, count, None)
                    .expect("line motions always land");
                let column = cursor - line_start(&chars, cursor);
                let (start, end) = line_bounds(&chars, line);
                (start + column).min(end)
            }
            Motion::FirstLine | Motion::LastLine => {
                let line = motion_target(&chars, cursor, motion, count, pending.explicit_count())
                    .expect("line motions always land");
                first_non_blank(&chars, line)
            }
            // A forward move into a paste marker passes over it; the marker
            // is one atomic character.
            _ => match motion_target(&chars, cursor, motion, count, None) {
                Some(target) if target > cursor => buffer.caret_index_after(target),
                Some(target) => target,
                None => return VimOutcome::Handled(TextChange::Unchanged),
            },
        };
        buffer.set_cursor(buffer.caret_index(target));
        clamp_to_char(buffer);
        VimOutcome::Handled(TextChange::Unchanged)
    }

    fn operate_motion(
        &mut self,
        operator: Operator,
        motion: Motion,
        count: usize,
        buffer: &mut ComposerBuffer,
    ) -> VimOutcome {
        let pending = Pending {
            count: Some(count),
            operator: Some(operator),
            ..Pending::default()
        };
        self.motion(pending, motion, buffer)
    }

    /// Apply `operator` to `count` lines starting at the caret's line.
    fn operate_lines(
        &mut self,
        operator: Operator,
        count: usize,
        buffer: &mut ComposerBuffer,
    ) -> VimOutcome {
        let chars = chars(buffer);
        let last = line_starts(&chars).len() - 1;
        let first = line_index(&chars, buffer.cursor());
        let target = Target::Lines {
            first,
            last: first.saturating_add(count - 1).min(last),
        };
        self.operate(operator, target, buffer)
    }

    fn operate(
        &mut self,
        operator: Operator,
        target: Target,
        buffer: &mut ComposerBuffer,
    ) -> VimOutcome {
        let chars = chars(buffer);
        let cursor = buffer.cursor();
        let (range, register_range, linewise) = match target {
            Target::Chars(range) => (range.clone(), range, false),
            Target::Lines { first, last } => {
                let (start, end) = line_text_range(&chars, first, last);
                (line_range(&chars, operator, first, last), start..end, true)
            }
        };
        if operator == Operator::Yank && (linewise || !range.is_empty()) {
            self.register = Some(Register {
                fragment: buffer.fragment(register_range),
                linewise,
            });
        } else if operator != Operator::Yank && !range.is_empty() {
            let mut fragment = buffer.replace_range(range.clone(), Fragment::default());
            // Line deletes also consume one separating newline. The register
            // contains only the lines, so put can supply its own separator.
            if linewise {
                if range.start < register_range.start {
                    fragment.text.remove(0);
                    for segment in &mut fragment.segments {
                        segment.start -= 1;
                    }
                } else if range.end > register_range.end {
                    fragment.text.pop();
                }
            }
            self.register = Some(Register { fragment, linewise });
        } else if linewise {
            self.register = Some(Register {
                fragment: Fragment::default(),
                linewise,
            });
        }
        match operator {
            Operator::Yank => {
                // Yanking backward leaves the caret at the start of the text.
                if range.start < cursor {
                    let target = if linewise {
                        let column = cursor - line_start(&chars, cursor);
                        let (start, end) = line_bounds(&chars, range.start);
                        (start + column).min(end)
                    } else {
                        range.start
                    };
                    buffer.set_cursor(buffer.caret_index(target));
                }
                clamp_to_char(buffer);
                VimOutcome::Handled(TextChange::Unchanged)
            }
            Operator::Delete => {
                if range.is_empty() {
                    return VimOutcome::Handled(TextChange::Unchanged);
                }
                if linewise {
                    let chars = self::chars(buffer);
                    buffer.set_cursor(first_non_blank(&chars, range.start.min(chars.len())));
                }
                clamp_to_char(buffer);
                VimOutcome::Handled(TextChange::Edited)
            }
            Operator::Change => {
                self.enter_insert();
                if range.is_empty() {
                    buffer.set_cursor(range.start);
                    return VimOutcome::Handled(TextChange::Unchanged);
                }
                VimOutcome::Handled(TextChange::Edited)
            }
        }
    }

    fn put(&mut self, placement: Placement, buffer: &mut ComposerBuffer) -> VimOutcome {
        let Some(register) = self.register.clone() else {
            return VimOutcome::Handled(TextChange::Unchanged);
        };
        let chars = chars(buffer);
        let cursor = buffer.cursor();
        let (start, end) = line_bounds(&chars, cursor);
        if register.linewise {
            let line = match placement {
                Placement::Before => {
                    buffer.replace_range(start..start, Fragment::plain("\n"));
                    start
                }
                Placement::After => {
                    buffer.replace_range(end..end, Fragment::plain("\n"));
                    end + 1
                }
            };
            let at = buffer.caret_index(line);
            buffer.replace_range(at..at, register.fragment);
            let chars = self::chars(buffer);
            buffer.set_cursor(first_non_blank(&chars, line));
        } else {
            let at = match placement {
                Placement::After if end > start => buffer.caret_index_after((cursor + 1).min(end)),
                Placement::After | Placement::Before => cursor,
            };
            let at = buffer.caret_index(at);
            buffer.replace_range(at..at, register.fragment);
            let last = buffer.cursor().saturating_sub(1).max(at);
            buffer.set_cursor(buffer.caret_index(last));
        }
        clamp_to_char(buffer);
        VimOutcome::Handled(TextChange::Edited)
    }
}

fn operator_char(operator: Operator) -> char {
    match operator {
        Operator::Delete => 'd',
        Operator::Change => 'c',
        Operator::Yank => 'y',
    }
}

/// `r`: overwrite `count` chars from the caret, all within its line.
fn replace_chars(buffer: &mut ComposerBuffer, ch: char, count: usize) -> VimOutcome {
    let chars = chars(buffer);
    let cursor = buffer.cursor();
    let (_, end) = line_bounds(&chars, cursor);
    if end.saturating_sub(cursor) < count {
        return VimOutcome::Handled(TextChange::Unchanged);
    }
    let replacement: String = std::iter::repeat_n(ch, count).collect();
    buffer.replace_range(cursor..cursor + count, Fragment::plain(replacement));
    buffer.set_cursor(buffer.caret_index(cursor + count - 1));
    clamp_to_char(buffer);
    VimOutcome::Handled(TextChange::Edited)
}

fn chars(buffer: &ComposerBuffer) -> Vec<char> {
    buffer.text().chars().collect()
}

/// Keep the caret on a character: never past the end of a non-empty line.
fn clamp_to_char(buffer: &mut ComposerBuffer) {
    buffer.set_cursor(normal_cursor(buffer));
}

/// Project an arbitrary caret onto the character normal mode operates on.
pub(super) fn normal_cursor(buffer: &ComposerBuffer) -> usize {
    let chars = chars(buffer);
    let cursor = buffer.cursor().min(chars.len());
    let (start, end) = line_bounds(&chars, cursor);
    let cursor = if cursor >= end && end > start {
        end - 1
    } else {
        cursor
    };
    buffer.caret_index(cursor)
}

#[cfg(test)]
#[path = "composer_vim_tests.rs"]
mod tests;
