use std::io::IsTerminal;

use crossterm::event::Event;
use ratatui::DefaultTerminal;

use super::{
    event_adapter::WorkflowEventAdapter,
    input::{handle_key, InputResult},
    state::WorkflowUiState,
    view,
};
use crate::tui::terminal_events::TerminalEvents;

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
    let _terminal_restore = RestoreTerminal;
    let session = adapter.session();
    let initial = adapter.initial_snapshot();
    let mut app = WorkflowUiState::new(session, initial);
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
    terminal.draw(|frame| view::draw(frame, app))?;

    loop {
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
                        InputResult::Exit => return Ok(WorkflowTuiExit::LeftScreen),
                    },
                    Event::Resize(_, _) => {
                        terminal.draw(|frame| view::draw(frame, app))?;
                    }
                    _ => {}
                }
            }
            update = adapter.next_event() => {
                let Some(update) = update? else {
                    if app.can_exit() {
                        return Ok(WorkflowTuiExit::LeftScreen);
                    }
                    anyhow::bail!("workflow event source ended before the run reached a durable state");
                };
                app.apply(update);
                terminal.draw(|frame| view::draw(frame, app))?;
            }
        }
    }
}

struct RestoreTerminal;

impl Drop for RestoreTerminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}
