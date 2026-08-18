//! Setup state for a session, and the header copy that follows from it.
//!
//! One fact decides how a running session presents itself: whether the active
//! provider has usable credentials. The session header and the statusline badge
//! read the same [`SetupState`], so a login or a logout changes them together
//! instead of letting them drift apart.
//!
//! [`SetupEntry`] is the separate question of whether this launch opens the
//! full-screen setup at all, and at which step. See [`super::setup_screen`].

use ratatui::{style::Style, text::Span};

use super::theme::Theme;

/// How much weight a header hint carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HintTone {
    /// Reference material the user can read at leisure.
    Reference,
    /// The one step that unblocks the session.
    NextStep,
}

impl HintTone {
    fn style(self) -> Style {
        match self {
            Self::Reference => Theme::dim(),
            Self::NextStep => Theme::accent(),
        }
    }
}

/// One line of the header hint block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Hint {
    pub(super) text: &'static str,
    pub(super) tone: HintTone,
}

impl Hint {
    const fn reference(text: &'static str) -> Self {
        Self {
            text,
            tone: HintTone::Reference,
        }
    }

    const fn next_step(text: &'static str) -> Self {
        Self {
            text,
            tone: HintTone::NextStep,
        }
    }

    pub(super) fn style(self) -> Style {
        self.tone.style()
    }
}

/// Hints for a session that can already run a turn.
const READY_HINTS: &[Hint] = &[
    Hint::reference(" shift+tab    Cycle reasoning level"),
    Hint::reference(" ctrl+p       Cycle pinned models"),
    Hint::reference(" ctrl+c       Clear the composer"),
    Hint::reference(" /            Show available commands"),
    Hint::reference(" !            Run a shell command"),
];

/// Hints for a session with no usable credentials. Login leads, because every
/// other action fails until it succeeds.
const SIGNED_OUT_HINTS: &[Hint] = &[
    Hint::next_step(" /login       Sign in to a provider"),
    Hint::reference(" /model       Choose a model"),
    Hint::reference(" /            Show available commands"),
];

/// The one headline the session header can carry. A first launch says its
/// welcome on the setup screen, so the header never repeats it.
const SIGNED_OUT_HEADLINE: &str = " Not signed in. Rho needs a provider before it can answer.";

/// Which step a launch opens the first-run setup screen at.
///
/// A real first launch asks for [`SetupEntry::Auto`]. The named steps exist so
/// the flow can be reviewed on a machine that is already configured, where
/// `Auto` would skip sign-in because models are already available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetupEntry {
    /// Whichever step can do something.
    Auto,
    /// Sign-in, even when the session could already list models.
    SignIn,
    /// Model choice, even when no login has happened on this machine.
    ChooseModel,
}

/// Where a session sits in provider setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SetupState {
    /// The active provider resolved to usable credentials.
    pub(super) signed_in: bool,
}

impl Default for SetupState {
    /// A session that can run a turn, which is what most sessions are.
    fn default() -> Self {
        Self { signed_in: true }
    }
}

impl SetupState {
    /// Copy above the hint block, shown only when the session cannot run a
    /// turn. A session that works needs no announcement.
    pub(super) fn headline(self) -> Option<Span<'static>> {
        (!self.signed_in).then(|| Span::styled(SIGNED_OUT_HEADLINE, Theme::warning()))
    }

    pub(super) fn hints(self) -> &'static [Hint] {
        if self.signed_in {
            READY_HINTS
        } else {
            SIGNED_OUT_HINTS
        }
    }
}

impl super::App {
    /// The session's live setup state. Login and logout clear or set
    /// `auth_unavailable`, so every read reflects the current credentials.
    pub(super) fn setup_state(&self) -> SetupState {
        SetupState {
            signed_in: self.info.services.auth_unavailable.is_none(),
        }
    }

    /// A prompt submitted with no credentials opens the login picker instead of
    /// failing a turn, and the composer holds the prompt so one enter sends it
    /// once a provider is live.
    ///
    /// The prompt is written back rather than left alone, because a template or
    /// skill command clears the composer while it expands. Writing back the
    /// resolved text keeps that work instead of dropping it, and it is the text
    /// the turn would have sent.
    pub(super) fn offer_login_instead_of_turn(
        &mut self,
        turn: super::TurnPrompt,
    ) -> anyhow::Result<()> {
        self.input_ui.set_text(turn.display);
        self.input_ui.set_cursor(self.input_ui.char_len());
        // Open the picker first; it sets its own chrome status, then replace
        // the toast with the held-prompt notice the user needs to see.
        self.open_login_picker();
        self.notify_status("not signed in yet; your prompt is still in the composer");
        Ok(())
    }

    /// Point a freshly signed-in user back at the prompt they were holding.
    pub(super) fn announce_held_prompt_after_login(&mut self) {
        if self.setup_state().signed_in && !self.input_ui.text().trim().is_empty() {
            self.notify_status("press enter to send the prompt you were holding");
        }
    }
}

#[cfg(test)]
#[path = "first_run_tests.rs"]
mod tests;
