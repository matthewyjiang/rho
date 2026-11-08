//! The rules a title a person gives a session must meet. Every rename goes
//! through [`super::Session::set_title`]: `rho sessions rename`, the TUI's
//! `/title`, and the daemon's `rho/session/rename` (#1667). Generated
//! titles are held to their own, tighter rules (`crate::title`).

/// The longest title a session takes, in characters.
//
// A tripwire, not a display budget: pickers cut titles to 80 characters
// anyway. Measured on a dev machine's session index (308 sessions, 5
// titled): titles run 33 characters at the median and 80 at most, the
// generated-title cap (`crate::title`'s `TITLE_MAX_CHARS`). Over six times
// that leaves any title a person types; only a paste of something that is
// not a title reaches it.
pub(super) const MAX_TITLE_CHARS: usize = 500;

/// Why a session refused a title.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TitleRejected {
    #[error("title must not be empty")]
    Empty,
    /// A newline, escape, or other control character, which would break
    /// the one line a list shows the title on, or drive the terminal.
    #[error("title must not contain control characters, but has {found:?}")]
    ControlCharacter { found: char },
    #[error(
        "title is {chars} characters, over the session title budget (MAX_TITLE_CHARS = {MAX_TITLE_CHARS})"
    )]
    TooLong { chars: usize },
}

/// `title` with surrounding whitespace trimmed, as a session saves it, or
/// why it cannot be one.
pub(super) fn checked(title: &str) -> Result<&str, TitleRejected> {
    let title = title.trim();
    if title.is_empty() {
        return Err(TitleRejected::Empty);
    }
    if let Some(found) = title.chars().find(|character| character.is_control()) {
        return Err(TitleRejected::ControlCharacter { found });
    }
    let chars = title.chars().count();
    if chars > MAX_TITLE_CHARS {
        return Err(TitleRejected::TooLong { chars });
    }
    Ok(title)
}
