//! Who owns the full terminal: setup, in-place attach, process peek, or the session.
//!
//! This is stored state, not a query over sibling flags. Setup, attach, and
//! peek all replace session chrome. Attach and peek swallow input; setup still
//! types into its composer pickers.

use std::time::Instant;

use crossterm::event::Event;
use ratatui::Frame;

use super::{
    attachment::AttachmentApp, process_peek::ProcessPeekView, setup_screen::SetupStep, App,
};

/// Foreground occupant of the interactive TUI.
pub(super) enum ExclusiveOccupant {
    Session,
    Setup(SetupStep),
    Attach {
        view: Box<AttachmentApp>,
        parent_turn_armed: bool,
    },
    Peek {
        view: Box<ProcessPeekView>,
    },
}

impl ExclusiveOccupant {
    pub(super) fn setup_step(&self) -> Option<SetupStep> {
        match self {
            Self::Setup(step) => Some(*step),
            Self::Session | Self::Attach { .. } | Self::Peek { .. } => None,
        }
    }

    /// Attach reads the journal; peek follows live process output. Both need
    /// the 100ms tick so the exclusive screen keeps moving.
    pub(super) fn wants_fast_ticks(&self) -> bool {
        matches!(self, Self::Attach { .. } | Self::Peek { .. })
    }

    pub(super) fn parent_turn_armed(&self) -> Option<bool> {
        match self {
            Self::Attach {
                parent_turn_armed, ..
            } => Some(*parent_turn_armed),
            Self::Session | Self::Setup(_) | Self::Peek { .. } => None,
        }
    }

    pub(super) fn attach_view(&self) -> Option<&AttachmentApp> {
        match self {
            Self::Attach { view, .. } => Some(view),
            Self::Session | Self::Setup(_) | Self::Peek { .. } => None,
        }
    }

    pub(super) fn attach_view_mut(&mut self) -> Option<&mut AttachmentApp> {
        match self {
            Self::Attach { view, .. } => Some(view),
            Self::Session | Self::Setup(_) | Self::Peek { .. } => None,
        }
    }

    pub(super) fn peek_view_mut(&mut self) -> Option<&mut ProcessPeekView> {
        match self {
            Self::Peek { view } => Some(view),
            Self::Session | Self::Setup(_) | Self::Attach { .. } => None,
        }
    }
}

impl App {
    pub(super) fn draw_exclusive_screen(&mut self, frame: &mut Frame<'_>) -> bool {
        match self.exclusive {
            ExclusiveOccupant::Setup(step) => {
                let area = frame.area();
                self.draw_setup_screen(frame, area, step);
                true
            }
            ExclusiveOccupant::Attach { .. } => self.draw_attach_screen(frame),
            ExclusiveOccupant::Peek { .. } => self.draw_peek_screen(frame),
            ExclusiveOccupant::Session => false,
        }
    }

    /// Consume a terminal event when the occupant owns the keyboard.
    ///
    /// Setup is exclusive paint only, so its events are returned for the
    /// composer. Attach and peek consume every event and report whether it was
    /// a resize.
    pub(super) fn take_exclusive_event(&mut self, event: Event) -> Result<bool, Event> {
        match self.exclusive {
            ExclusiveOccupant::Attach { .. } => Ok(self.route_attach_event(event)),
            ExclusiveOccupant::Peek { .. } => Ok(self.route_peek_event(event)),
            ExclusiveOccupant::Setup(_) | ExclusiveOccupant::Session => Err(event),
        }
    }

    pub(super) fn exclusive_should_redraw(&self, now: Instant) -> bool {
        match &self.exclusive {
            ExclusiveOccupant::Attach { view, .. } => view.should_redraw(now),
            ExclusiveOccupant::Peek { view } => view.should_redraw(now),
            ExclusiveOccupant::Session | ExclusiveOccupant::Setup(_) => false,
        }
    }

    pub(super) fn refresh_exclusive_screen(&mut self) -> anyhow::Result<bool> {
        match &mut self.exclusive {
            ExclusiveOccupant::Attach { view, .. } => {
                let changed = view.refresh()?;
                Ok(changed || view.should_redraw(Instant::now()))
            }
            ExclusiveOccupant::Peek { view } => {
                Ok(view.refresh() || view.should_redraw(Instant::now()))
            }
            ExclusiveOccupant::Session | ExclusiveOccupant::Setup(_) => Ok(false),
        }
    }
}
