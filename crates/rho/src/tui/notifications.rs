//! Desktop notifications for when a turn finishes or needs the user while the
//! terminal is unfocused.
//!
//! The TUI feeds every agent state it reports to Herdr through
//! [`TerminalNotifier::observe`]. A wait for the user (approval, questionnaire,
//! blocked goal) notifies at once. A finished turn is held until the event loop
//! is about to wait for input ([`TerminalNotifier::take_ready`]), so a goal run
//! or queued follow-ups notify once at the end instead of after every turn.
//!
//! Under Herdr the notifier stays off: Herdr receives the same states and owns
//! notifications for its panes.

use std::io::{self, Write};

use crate::herdr::HerdrState;

/// How the host terminal is asked to get the user's attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NotificationChannel {
    /// OSC 9 desktop notification (iTerm2, WezTerm, Ghostty, kitty).
    Osc9,
    /// Plain BEL. Terminals and multiplexers map it to a sound, dock bounce,
    /// or tab marker.
    Bell,
}

impl NotificationChannel {
    /// Picks a channel from the terminal's environment. Multiplexers come
    /// first: they swallow OSC 9 but forward BEL, and they inherit variables
    /// such as `KITTY_WINDOW_ID` from the outer terminal.
    pub(super) fn detect(get_var: impl Fn(&str) -> Option<String>) -> Self {
        let var = |key| get_var(key).unwrap_or_default();
        if !var("TMUX").is_empty() || !var("STY").is_empty() {
            return Self::Bell;
        }
        if !var("KITTY_WINDOW_ID").is_empty() {
            return Self::Osc9;
        }
        let osc9 = matches!(
            var("TERM_PROGRAM").as_str(),
            "iTerm.app" | "WezTerm" | "ghostty"
        ) || matches!(var("TERM").as_str(), "xterm-kitty" | "xterm-ghostty");
        if osc9 {
            Self::Osc9
        } else {
            Self::Bell
        }
    }

    /// Bytes that deliver `body` on this channel. Control characters in the
    /// body are replaced so text such as a goal's blocked reason cannot end
    /// the OSC sequence early or inject another one.
    pub(super) fn encode(self, body: &str) -> Vec<u8> {
        match self {
            Self::Osc9 => {
                let body: String = body
                    .chars()
                    .map(|ch| if ch.is_control() { ' ' } else { ch })
                    .collect();
                format!("\x1b]9;rho: {body}\x07").into_bytes()
            }
            Self::Bell => b"\x07".to_vec(),
        }
    }
}

/// Decides when to notify. Owned by the TUI app; holds no terminal handle.
#[derive(Debug)]
pub(super) struct TerminalNotifier {
    /// `None` when another host (Herdr) owns notifications.
    channel: Option<NotificationChannel>,
    /// Terminals report focus changes, not the initial focus. The user just
    /// launched Rho, so start focused; terminals without focus reporting
    /// never notify.
    focused: bool,
    last: Option<(HerdrState, Option<String>)>,
    /// A finished turn waiting for the event loop to go idle.
    finished_turn: bool,
}

impl TerminalNotifier {
    pub(super) fn new(channel: Option<NotificationChannel>) -> Self {
        Self {
            channel,
            focused: true,
            last: None,
            finished_turn: false,
        }
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Records a reported agent state. Returns bytes to write when the state
    /// is a new wait for the user and the terminal is unfocused.
    pub(super) fn observe(&mut self, state: HerdrState, message: Option<&str>) -> Option<Vec<u8>> {
        let next = (state, message.map(str::to_string));
        if self.last.as_ref() == Some(&next) {
            return None;
        }
        let was_working = matches!(self.last, Some((HerdrState::Working, _)));
        self.last = Some(next);
        match state {
            HerdrState::Working => {
                self.finished_turn = false;
                None
            }
            HerdrState::Idle => {
                self.finished_turn |= was_working;
                None
            }
            HerdrState::Blocked => {
                // A blocked rest after a turn replaces the finished-turn note.
                self.finished_turn = false;
                self.encode(message.unwrap_or("waiting for you"))
            }
        }
    }

    /// Bytes for a finished turn once the event loop is about to wait for
    /// input. Clears the pending turn either way.
    pub(super) fn take_ready(&mut self) -> Option<Vec<u8>> {
        if !std::mem::take(&mut self.finished_turn) {
            return None;
        }
        self.encode("turn finished")
    }

    fn encode(&self, body: &str) -> Option<Vec<u8>> {
        if self.focused {
            return None;
        }
        Some(self.channel?.encode(body))
    }
}

/// Writes notification bytes straight to the terminal. OSC 9 and BEL do not
/// move the cursor, so this is safe between frames.
pub(super) fn write_to_terminal(bytes: &[u8]) {
    let mut stdout = io::stdout().lock();
    // Best effort: a lost notification is not worth failing the turn over.
    let _ = stdout.write_all(bytes).and_then(|()| stdout.flush());
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
