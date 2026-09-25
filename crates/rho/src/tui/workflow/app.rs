use std::{
    io::IsTerminal,
    time::{Duration, Instant},
};

use crossterm::event::Event;
use ratatui::DefaultTerminal;

use crate::clipboard::SystemClipboard;

use super::super::{
    mouse_capture, terminal_events::TerminalEvents, text_selection::CopyNotice, theme::Theme,
};
use super::{
    event_adapter::WorkflowEventAdapter,
    input::{handle_key, InputResult},
    state::WorkflowUiState,
    view,
};

/// Redraw cadence for timed chrome (details scrollbar, copy notice) without input.
const SCROLLBAR_TICK: Duration = Duration::from_millis(100);

/// Why the workflow screen closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkflowTuiExit {
    /// User left the screen, or the event source ended while leave was allowed.
    LeftScreen,
}

/// Runs the dedicated workflow screen.
///
/// The caller must select text or JSONL before this function when either
/// standard input or standard output is not a terminal.
pub(crate) async fn run(
    mut adapter: Box<dyn WorkflowEventAdapter>,
) -> anyhow::Result<WorkflowTuiExit> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "the workflow TUI requires an interactive terminal; use text or JSONL output"
        );
    }

    let mut terminal = ratatui::init();
    let _terminal_restore = RestoreTerminal {
        mouse_capture: mouse_capture::Guard::acquire(),
    };
    Theme::initialize_from_terminal();
    let session = adapter.session();
    let initial = adapter.initial_snapshot();
    let mut app = WorkflowUiState::new(session, initial, adapter.run_directory());
    match run_loop(&mut terminal, &mut app, adapter.as_mut()).await {
        Ok(exit) => Ok(exit),
        Err(run_error) => match adapter.shutdown().await {
            Ok(()) => Err(run_error),
            Err(shutdown_error) => {
                Err(run_error.context(format!("workflow cleanup also failed: {shutdown_error:#}")))
            }
        },
    }
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    app: &mut WorkflowUiState,
    adapter: &mut dyn WorkflowEventAdapter,
) -> anyhow::Result<WorkflowTuiExit> {
    let mut terminal_events = TerminalEvents::new();
    let mut clipboard = SystemClipboard::new();
    let mut scrollbar_tick = tokio::time::interval(SCROLLBAR_TICK);
    scrollbar_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    terminal.draw(|frame| view::draw(frame, app))?;

    loop {
        let now = Instant::now();
        // Transient chrome (scrollbar, copy notice) hides on a timer, not input.
        let transient_visible =
            app.details().should_render_scrollbar(now) || app.copy_notice().is_some();
        tokio::select! {
            terminal_event = terminal_events.next() => {
                match terminal_event? {
                    Event::Key(key) => match handle_key(app, key) {
                        InputResult::Ignore => {}
                        InputResult::Redraw => {
                            terminal.draw(|frame| view::draw(frame, app))?;
                        }
                        InputResult::Action(action) => {
                            adapter.send(action).await?;
                        }
                        InputResult::Exit => {
                            adapter.finish().await?;
                            return Ok(WorkflowTuiExit::LeftScreen);
                        }
                    },
                    Event::Mouse(mouse) => {
                        let mut redraw =
                            view::handle_mouse(app, mouse.kind, mouse.column, mouse.row);
                        if let Some(text) = app.details_mut().take_pending_copy() {
                            app.set_copy_notice(CopyNotice::from_copy_result(
                                clipboard.copy_text(&text),
                                text.chars().count(),
                                Instant::now(),
                            ));
                            redraw = true;
                        }
                        if redraw {
                            terminal.draw(|frame| view::draw(frame, app))?;
                        }
                    }
                    Event::Resize(_, _) => {
                        terminal.draw(|frame| view::draw(frame, app))?;
                    }
                    Event::FocusGained => {
                        mouse_capture::reassert();
                    }
                    _ => {}
                }
            }
            update = adapter.next_event() => {
                let Some(update) = update? else {
                    if app.can_exit() {
                        adapter.finish().await?;
                        return Ok(WorkflowTuiExit::LeftScreen);
                    }
                    anyhow::bail!("workflow event source ended before the run reached a durable state");
                };
                app.apply(update);
                terminal.draw(|frame| view::draw(frame, app))?;
            }
            _ = scrollbar_tick.tick(), if transient_visible => {
                // Drop the scrollbar after HISTORY_SCROLLBAR_REVEAL_DURATION and
                // the copy notice after its own duration, without input.
                app.expire_copy_notice(Instant::now());
                terminal.draw(|frame| view::draw(frame, app))?;
            }
        }
    }
}

struct RestoreTerminal {
    mouse_capture: mouse_capture::Guard,
}

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        self.mouse_capture.release();
        ratatui::restore();
    }
}
