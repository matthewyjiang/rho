//! Where the keyboard is: the composer, a control of a surface, or the
//! text of one of its fields.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::line_editor::LineEditor;

/// The keyboard's place among the surfaces. `index` counts the surface's
/// focusable controls, in the order they are drawn.
#[derive(Clone, Debug, Default)]
pub(in crate::tui) enum SurfaceFocus {
    /// On the composer.
    #[default]
    Idle,
    /// On a control: keys move among controls and operate them.
    Navigating { surface: String, index: usize },
    /// In a field: keys edit its text.
    Editing {
        surface: String,
        index: usize,
        editor: LineEditor,
        /// Why the text is not the field's value yet.
        invalid: Option<&'static str>,
    },
}

impl SurfaceFocus {
    /// The surface and control the keyboard is on.
    pub(super) fn place(&self) -> Option<(&str, usize)> {
        match self {
            Self::Idle => None,
            Self::Navigating { surface, index } | Self::Editing { surface, index, .. } => {
                Some((surface, *index))
            }
        }
    }
}

/// Whether `key` is the chord that moves the keyboard from the composer
/// to the latest surface.
pub(super) fn is_focus_chord(key: KeyEvent) -> bool {
    key.modifiers == KeyModifiers::ALT && key.code == KeyCode::Char('u')
}

/// What a key asks of the control the keyboard is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NavKey {
    Next,
    Previous,
    Left,
    Right,
    Activate,
    Leave,
    /// A character, which starts typing in a field.
    Type(char),
}

impl NavKey {
    /// The request `key` makes while the keyboard is on a control, or
    /// `None` for a key the surface leaves to the rest of the TUI.
    pub(super) fn of(key: KeyEvent) -> Option<Self> {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::BackTab => Some(Self::Previous),
            _ if !plain => None,
            KeyCode::Tab | KeyCode::Down => Some(Self::Next),
            KeyCode::Up => Some(Self::Previous),
            KeyCode::Left => Some(Self::Left),
            KeyCode::Right => Some(Self::Right),
            KeyCode::Enter | KeyCode::Char(' ') => Some(Self::Activate),
            KeyCode::Esc => Some(Self::Leave),
            KeyCode::Char(ch) => Some(Self::Type(ch)),
            _ => None,
        }
    }
}

/// What a key asks of the field being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EditKey {
    /// A change to the text or the caret, already applied.
    Edited,
    /// Stop editing and stay on the field.
    Done,
    /// Stop editing and move to the next or previous control.
    DoneThen(NavKey),
}

impl EditKey {
    /// Applies `key` to `editor`, or returns `None` for a key the field
    /// leaves to the rest of the TUI. A character the field does not
    /// `accept` is dropped: the composer has no use for it either while
    /// the keyboard is here.
    pub(super) fn apply(
        key: KeyEvent,
        editor: &mut LineEditor,
        accepts: impl Fn(char) -> bool,
    ) -> Option<Self> {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::BackTab => return Some(Self::DoneThen(NavKey::Previous)),
            _ if !plain => return None,
            KeyCode::Enter | KeyCode::Esc => return Some(Self::Done),
            KeyCode::Tab | KeyCode::Down => return Some(Self::DoneThen(NavKey::Next)),
            KeyCode::Up => return Some(Self::DoneThen(NavKey::Previous)),
            KeyCode::Char(ch) if accepts(ch) => editor.insert_char(ch),
            KeyCode::Char(_) => {}
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Left => editor.move_cursor_left(),
            KeyCode::Right => editor.move_cursor_right(),
            KeyCode::Home => editor.move_cursor_home(),
            KeyCode::End => editor.move_cursor_end(),
            _ => return None,
        }
        Some(Self::Edited)
    }
}
