//! A2UI surfaces in the transcript: what the session's `ui` calls showed,
//! drawn under each call's card and operated from the keyboard.
//!
//! [`store::SurfaceStore`] holds the session's A2UI processor, which every
//! `ui` call's messages go through once. [`tree`] reads one surface out of
//! it as a typed [`tree::SurfaceTree`]: what to draw, and the controls the
//! keyboard reaches, in order. [`render`] turns a tree into transcript
//! lines, and [`focus`] is where the keyboard is. A [`SurfaceEntry`] holds
//! a snapshot of its surface, so drawing the transcript stays a function
//! of its entries; `app` takes a new snapshot after every change.

use rho_sdk::ToolCallId;

mod app;
mod focus;
mod render;
mod store;
mod tree;

pub(super) use focus::SurfaceFocus;
pub(super) use render::surface_lines;
use store::SurfaceStore;
use tree::SurfaceTree;

/// A surface in the transcript, under the card of the call that showed it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SurfaceEntry {
    /// The `ui` call it is drawn under.
    pub(in crate::tui) call: ToolCallId,
    /// The surface's id, unique within the session.
    pub(in crate::tui) surface: String,
    /// What to draw. `None` draws nothing: the surface was deleted, or a
    /// later call showed it again and it is drawn under that call's card.
    pub(in crate::tui) view: Option<SurfaceView>,
}

/// One surface as it stands, and where the keyboard is on it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SurfaceView {
    tree: std::sync::Arc<SurfaceTree>,
    keyboard: Keyboard,
}

/// What the keyboard does to a surface, as its view shows it.
#[derive(Clone, Debug, PartialEq)]
enum Keyboard {
    /// Elsewhere. `reachable` when the focus chord moves it here.
    Away { reachable: bool },
    /// On control `index` of the tree's focusables.
    On {
        index: usize,
        editing: Option<Editing>,
    },
}

/// The text being typed into the focused field.
#[derive(Clone, Debug, PartialEq)]
struct Editing {
    text: String,
    /// The caret, in characters from the start.
    cursor: usize,
    /// Why `text` is not the field's value yet.
    invalid: Option<&'static str>,
}

/// The followed session's surfaces, and where the keyboard is on them.
#[derive(Default)]
pub(super) struct Surfaces {
    store: SurfaceStore,
    /// Surfaces of calls still in the live tool batch, drawn under their
    /// live cards. A call that closes takes its surface to the transcript.
    live: Vec<SurfaceEntry>,
    focus: SurfaceFocus,
}

impl Surfaces {
    /// The surfaces of calls still running, as the rows under their live
    /// cards, each followed by the blank row a transcript entry ends with.
    pub(super) fn live_lines(&self, width: usize) -> Vec<ratatui::text::Line<'static>> {
        use super::render::{pad_display_line, padded_content_width, styled_blank_line};
        let mut lines = Vec::new();
        for view in self.live.iter().filter_map(|entry| entry.view.as_ref()) {
            lines.extend(
                surface_lines(view, padded_content_width(width))
                    .into_iter()
                    .map(pad_display_line),
            );
            lines.push(styled_blank_line(width, ratatui::style::Style::default()));
        }
        lines
    }

    /// Whether the keyboard is on a surface, not the composer.
    pub(super) fn has_keyboard(&self) -> bool {
        !matches!(self.focus, SurfaceFocus::Idle)
    }

    /// The index in `entries`, the transcript, of the surface the keyboard
    /// is on. `None` while it is elsewhere or on a live call's surface.
    pub(super) fn operated_entry(&self, entries: &[super::Entry]) -> Option<usize> {
        let (surface, _) = self.focus.place()?;
        entries.iter().rposition(|entry| {
            matches!(entry, super::Entry::Surface(shown)
                if shown.surface == surface && shown.view.is_some())
        })
    }
}
