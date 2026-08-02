//! Setup state for a session, and the header copy that follows from it.
//!
//! Two facts decide how a session presents itself: whether Rho wrote the config
//! file during this launch (a first run), and whether the active provider has
//! usable credentials. Every setup-aware surface reads the same [`SetupState`],
//! so a login or a logout changes the session header and the statusline badge
//! together instead of drifting apart.

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

/// The headline above the hint block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Headline {
    text: &'static str,
    warning: bool,
}

impl Headline {
    pub(super) fn span(self) -> Span<'static> {
        let style = if self.warning {
            Theme::warning()
        } else {
            Theme::text_strong()
        };
        Span::styled(self.text, style)
    }
}

/// Where a session sits in provider setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SetupState {
    /// Rho created the config file during this launch.
    pub(super) first_run: bool,
    /// The active provider resolved to usable credentials.
    pub(super) signed_in: bool,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            first_run: false,
            signed_in: true,
        }
    }
}

impl SetupState {
    pub(super) fn headline(self) -> Option<Headline> {
        match (self.signed_in, self.first_run) {
            (false, true) => Some(Headline {
                text: " Welcome to Rho. Sign in to a provider to start.",
                warning: true,
            }),
            (false, false) => Some(Headline {
                text: " Not signed in. Rho needs a provider before it can answer.",
                warning: true,
            }),
            (true, true) => Some(Headline {
                text: " Welcome to Rho. Type a prompt and press enter.",
                warning: false,
            }),
            (true, false) => None,
        }
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
            first_run: self.info.services.first_run,
            signed_in: self.info.services.auth_unavailable.is_none(),
        }
    }

    /// A prompt submitted with no credentials opens the login picker instead of
    /// failing a turn. The composer keeps the text, so one enter sends it once
    /// a provider is live.
    pub(super) fn offer_login_instead_of_turn(&mut self) -> anyhow::Result<()> {
        self.notify_status("not signed in yet; your prompt is still in the composer");
        self.open_login_picker();
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
