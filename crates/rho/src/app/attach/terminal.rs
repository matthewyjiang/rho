//! The terminal side of a text-mode `rho attach` (#1669): the lines typed on
//! stdin and the console they go to, and the daemon requests the console's
//! actions make.

use std::{
    io::BufRead,
    time::{SystemTime, UNIX_EPOCH},
};

use rho_protocol::{
    methods::{SessionAnswer, SessionCancel, SessionHold, SessionSteer},
    Client, ErrorCode, InputRequest, Replay, SessionAnswerParams, SessionCancelParams,
    SessionEvent, SessionHoldParams, SessionId, SessionSteerParams,
};
use tokio::sync::mpsc;

use super::console::{Action, Console, Refusal, Typed};
use crate::{
    app::automation::{self, AutomationExit},
    cli::AttachInput,
    stdio,
};

/// Whether the lines typed on stdin are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lines {
    Read,
    /// Requests are only told: stdin is not this process's to read.
    Ignore,
}

impl Lines {
    /// Whether `input` has stdin read: a terminal's only while this process
    /// runs in its foreground, so a script's stdin, such as a list a `while
    /// read` loop works through, is not taken for answers, and a `rho
    /// attach` started in the background is not stopped for reading the
    /// terminal.
    pub(super) fn of(input: AttachInput) -> Self {
        match input {
            AttachInput::Stdin => Self::Read,
            AttachInput::Terminal if stdio::stdin_is_foreground_terminal() => Self::Read,
            AttachInput::Terminal => Self::Ignore,
        }
    }
}

/// The console, and the lines typed for it while there are any.
pub(super) struct Terminal {
    console: Console,
    lines: Option<mpsc::UnboundedReceiver<Line>>,
}

/// A line read from stdin, and when, as Unix time in milliseconds.
pub(super) struct Line {
    text: String,
    at_ms: u64,
}

impl Terminal {
    pub(super) fn open(lines: Lines) -> Result<Self, AutomationExit> {
        let mut console = Console::terminal();
        let lines = match lines {
            Lines::Read => Some(read_lines()),
            Lines::Ignore => {
                console.closed().map_err(automation::write_failed)?;
                None
            }
        };
        Ok(Self { console, lines })
    }

    /// Hands the console an attach response's open requests, after its
    /// `replay`.
    pub(super) fn attached(
        &mut self,
        inputs: &[InputRequest],
        replay: Replay,
    ) -> Result<(), AutomationExit> {
        self.console
            .attached(inputs, replay, now_ms())
            .map_err(automation::write_failed)
    }

    /// Hands the console a live event of the session.
    pub(super) fn event(&mut self, event: &SessionEvent) -> Result<(), AutomationExit> {
        self.console
            .event(event, now_ms())
            .map_err(automation::write_failed)
    }

    /// The next line typed. At the end of stdin the console only tells
    /// requests from then on, and this never returns again.
    pub(super) async fn next_line(&mut self) -> Result<Line, AutomationExit> {
        if let Some(lines) = &mut self.lines {
            if let Some(line) = lines.recv().await {
                return Ok(line);
            }
            self.lines = None;
            self.console.closed().map_err(automation::write_failed)?;
        }
        std::future::pending().await
    }

    /// Takes `line` typed for `session`, and makes the requests of the
    /// daemon it comes to.
    pub(super) async fn line(
        &mut self,
        client: &Client,
        session: &SessionId,
        line: &Line,
    ) -> Result<(), AutomationExit> {
        let typed = Typed {
            line: &line.text,
            at_ms: line.at_ms,
        };
        let actions = self
            .console
            .line(typed, now_ms())
            .map_err(automation::write_failed)?;
        for action in actions {
            self.perform(client, session, action).await?;
        }
        Ok(())
    }

    /// Asks the daemon what `action` asks, and tells the console when the
    /// daemon refuses it.
    async fn perform(
        &mut self,
        client: &Client,
        session: &SessionId,
        action: Action,
    ) -> Result<(), AutomationExit> {
        let session = session.clone();
        let sent = match &action {
            Action::Hold(request) => client
                .request::<SessionHold>(&SessionHoldParams {
                    session,
                    request: *request,
                })
                .await
                .map(drop),
            Action::Answer(request, answer) => client
                .request::<SessionAnswer>(&SessionAnswerParams {
                    session,
                    request: *request,
                    answer: answer.clone(),
                })
                .await
                .map(drop),
            Action::Steer(text) => client
                .request::<SessionSteer>(&SessionSteerParams {
                    session,
                    text: text.clone(),
                })
                .await
                .map(drop),
            Action::Cancel => client
                .request::<SessionCancel>(&SessionCancelParams { session })
                .await
                .map(drop),
        };
        let Err(error) = sent else {
            return Ok(());
        };
        let notice = match (action, error.error_code()) {
            // A request that closed meanwhile needs no hold; its resolution
            // tells how it closed.
            (Action::Hold(_), Some(ErrorCode::InputNotOpen)) => return Ok(()),
            (Action::Hold(_), _) => format!("could not hold the request: {error}"),
            (Action::Answer(request, _), code) => {
                let refusal = if code == Some(ErrorCode::InputNotOpen) {
                    Refusal::NotOpen
                } else {
                    Refusal::Other(error.to_string())
                };
                return self
                    .console
                    .refused(request, refusal, now_ms())
                    .map_err(automation::write_failed);
            }
            (Action::Steer(_), Some(ErrorCode::NotSteerable)) => {
                "could not steer the turn: no model request is taking input".to_owned()
            }
            (Action::Steer(_), _) => format!("could not steer the turn: {error}"),
            (Action::Cancel, _) => format!("could not cancel the turn: {error}"),
        };
        self.console.tell(&notice).map_err(automation::write_failed)
    }
}

/// Reads stdin's lines on a thread of their own, which a blocking read
/// cannot hold the process open from; the channel closes at the end of
/// input. A line that is not UTF-8 is read with replacement characters.
fn read_lines() -> mpsc::UnboundedReceiver<Line> {
    let (lines, receiver) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut line = Vec::new();
        loop {
            line.clear();
            match stdin.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let text = String::from_utf8_lossy(&line)
                        .trim_end_matches(['\n', '\r'])
                        .to_owned();
                    if lines
                        .send(Line {
                            text,
                            at_ms: now_ms(),
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    });
    receiver
}

/// Now, as Unix time in milliseconds, which input timeouts count in.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
