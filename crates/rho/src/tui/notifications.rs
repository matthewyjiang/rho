//! Desktop notifications for when a turn finishes or needs the user while the
//! terminal is unfocused.
//!
//! Owners report attention events directly: approval and questionnaire
//! prompts call [`TerminalNotifier::user_wait`], which notifies at once, and
//! each finished turn calls [`TerminalNotifier::turn_finished`]. A finished
//! turn is held until the event loop is about to wait for input
//! ([`TerminalNotifier::take_ready`]), so a goal run or queued follow-ups
//! notify once at the end instead of after every turn.

use std::io::{self, Write};

use crossterm::event::Event;

use super::UserWait;

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

    /// Bytes that deliver `body` on this channel.
    pub(super) fn encode(self, body: &str) -> Vec<u8> {
        match self {
            Self::Osc9 => format!("\x1b]9;rho: {body}\x07").into_bytes(),
            Self::Bell => b"\x07".to_vec(),
        }
    }
}

/// Decides when to notify. Owned by the TUI app; holds no terminal handle.
/// Whether notifications are enabled at all is the caller's policy.
#[derive(Debug)]
pub(super) struct TerminalNotifier {
    channel: NotificationChannel,
    /// Terminals report focus changes, not the initial focus. The user just
    /// launched Rho, so start focused; terminals without focus reporting
    /// never notify.
    focused: bool,
    /// A turn finished since the event loop last waited for input.
    finished_turn: bool,
}

impl TerminalNotifier {
    pub(super) fn new(channel: NotificationChannel) -> Self {
        Self {
            channel,
            focused: true,
            finished_turn: false,
        }
    }

    /// Tracks terminal focus. Call for every terminal event before any
    /// exclusive screen consumes it.
    pub(super) fn observe_focus(&mut self, event: &Event) {
        match event {
            Event::FocusGained => self.focused = true,
            Event::FocusLost => self.focused = false,
            Event::Key(_) | Event::Mouse(_) | Event::Paste(_) | Event::Resize(..) => {}
        }
    }

    pub(super) fn turn_finished(&mut self) {
        self.finished_turn = true;
    }

    /// Bytes for a prompt that now waits on the user.
    pub(super) fn user_wait(&self, wait: UserWait) -> Option<Vec<u8>> {
        self.encode(wait.message())
    }

    /// Bytes for finished turns once the event loop is about to wait for
    /// input. Clears them either way.
    pub(super) fn take_ready(&mut self) -> Option<Vec<u8>> {
        if !std::mem::take(&mut self.finished_turn) {
            return None;
        }
        self.encode("turn finished")
    }

    fn encode(&self, body: &str) -> Option<Vec<u8>> {
        (!self.focused).then(|| self.channel.encode(body))
    }
}

/// Writes cursor-neutral terminal reports straight to the terminal. OSC 9,
/// OSC 7501, and BEL do not move the cursor, so this is safe between frames.
pub(super) fn write_to_terminal(bytes: &[u8]) {
    let mut stdout = io::stdout().lock();
    // Best effort: a lost report is not worth failing the turn over.
    let _ = stdout.write_all(bytes).and_then(|()| stdout.flush());
}

#[cfg(test)]
#[path = "notifications_tests.rs"]
mod tests;
