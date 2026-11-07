//! `rho attach`'s side of a session's input requests in text mode (#1669):
//! it tells the user each request the turn waits for and, while lines can
//! be read, answers the open requests one at a time, oldest first. A line
//! typed while nothing is asked steers the running turn; `/cancel` cancels
//! it at any point.
//!
//! A line means what was on screen when it was typed: one typed before the
//! request now asked was shown, or before the one it was typed for closed,
//! is dropped rather than answering something its typist never saw.
//!
//! Every field a request carries, written by the model or another party,
//! is escaped where it is formatted: a field shown on one line cannot break
//! it and pass for rho's own lines, and none can steer the terminal.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};

use rho_protocol::{
    Ask, InputAnswer, InputRequest, InputRequestId, Operation, PathScope, Replay, Resolution,
    SessionEvent,
};

use super::prompt::{Prompt, Reply};
use crate::control_chars::{escape_controls, LineBreaks};

/// What the console asks of the daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// Stop the request's timeout: the user has started on it.
    Hold(InputRequestId),
    Answer(InputRequestId, InputAnswer),
    Steer(String),
    Cancel,
}

/// Why the daemon refused an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The request closed before the answer reached it.
    NotOpen,
    /// The daemon would not take it, for this reason; the request stays
    /// open.
    Other(String),
}

/// A line the user typed, and when it was read, as Unix time in
/// milliseconds.
pub(super) struct Typed<'a> {
    pub(super) line: &'a str,
    pub(super) at_ms: u64,
}

/// The session's open input requests, as one attachment answers them.
pub(super) struct Console<E = io::Stderr> {
    err: E,
    /// The open requests, by id, which the daemon counts up, so oldest
    /// first.
    open: BTreeMap<InputRequestId, InputRequest>,
    /// Requests this console answered whose resolution has not come yet.
    answered: BTreeSet<InputRequestId>,
    /// Requests the daemon says were held, by any client, so they no longer
    /// time out.
    held: BTreeSet<InputRequestId>,
    input: Input,
    /// When `input` began, as Unix time in milliseconds: a line read before
    /// then was typed for what was shown before.
    since_ms: u64,
}

/// Whether lines are read, and what the next one is for.
enum Input {
    /// Nothing is asked: a line steers the turn.
    Free,
    /// A request is being answered.
    Answering(Prompt),
    /// No more lines can be read: requests are only told.
    Closed,
}

impl Console {
    pub(super) fn terminal() -> Self {
        Self::new(io::stderr())
    }
}

impl<E: Write> Console<E> {
    pub(super) fn new(err: E) -> Self {
        Self {
            err,
            open: BTreeMap::new(),
            answered: BTreeSet::new(),
            held: BTreeSet::new(),
            input: Input::Free,
            since_ms: 0,
        }
    }

    /// Takes an attach response's open requests, as of `now_ms`, once its
    /// replay is written: they replace the ones known, telling those that
    /// arrived and those that closed meanwhile. After a `replay` that
    /// resets, which wrote the conversation again, the request being
    /// answered is shown again.
    pub(super) fn attached(
        &mut self,
        inputs: &[InputRequest],
        replay: Replay,
        now_ms: u64,
    ) -> io::Result<()> {
        let gone: Vec<InputRequestId> = self
            .open
            .keys()
            .filter(|id| inputs.iter().all(|request| request.id != **id))
            .copied()
            .collect();
        for id in gone {
            self.close(id, "it closed while this client caught up", now_ms)?;
        }
        if let (Replay::Reset, Input::Answering(prompt)) = (replay, &self.input) {
            let text = format!(
                "rho: the turn waits for {}\n{}",
                asked(&self.open[&prompt.id()].ask),
                // Whole, a plan approval's plan included: the screen was
                // written again without it.
                prompt.opening()
            );
            self.say(&text)?;
        }
        for request in inputs {
            match self.open.get_mut(&request.id) {
                // Known: only its timeout can have changed, which a hold
                // while this client caught up stopped.
                Some(known) => {
                    if known.timeout.is_some() && request.timeout.is_none() {
                        self.held.insert(request.id);
                    }
                    known.timeout = request.timeout.clone();
                }
                None => self.arrived(request.clone(), now_ms)?,
            }
        }
        self.advance(now_ms)
    }

    /// Takes a live event of the session; replayed ones are history the
    /// attach response's open requests already account for.
    pub(super) fn event(&mut self, event: &SessionEvent, now_ms: u64) -> io::Result<()> {
        match event {
            SessionEvent::InputRequested { request } if !self.open.contains_key(&request.id) => {
                self.arrived((**request).clone(), now_ms)?;
            }
            SessionEvent::InputResolved { id, resolution } => {
                self.close(*id, resolved(*resolution), now_ms)?;
            }
            SessionEvent::InputHeld { id } => {
                if let Some(request) = self.open.get_mut(id) {
                    request.timeout = None;
                    self.held.insert(*id);
                }
            }
            _ => return Ok(()),
        }
        self.advance(now_ms)
    }

    /// Takes a line the user typed, and returns what to ask of the daemon.
    pub(super) fn line(&mut self, typed: Typed<'_>, now_ms: u64) -> io::Result<Vec<Action>> {
        let line = typed.line.trim();
        if line == "/cancel" {
            return Ok(vec![Action::Cancel]);
        }
        if typed.at_ms < self.since_ms {
            if !line.is_empty() {
                let shown = match self.input {
                    Input::Answering(_) => "before this request was asked",
                    Input::Free | Input::Closed => "for a request no longer asked",
                };
                self.tell(&format!("ignored \"{line}\", typed {shown}"))?;
            }
            return Ok(Vec::new());
        }
        match &mut self.input {
            Input::Closed => Ok(Vec::new()),
            Input::Free => match line {
                "" => Ok(Vec::new()),
                // A slip of a command, such as `/cancle`, is not sent to the
                // model; a path, which has more than one word, is.
                command if is_command(command) => {
                    self.tell(&format!(
                        "{command} is not a command here; /cancel cancels the turn"
                    ))?;
                    Ok(Vec::new())
                }
                text => Ok(vec![Action::Steer(text.to_owned())]),
            },
            Input::Answering(prompt) => {
                let id = prompt.id();
                if line == "/hold" {
                    return self.hold_command(id);
                }
                match prompt.reply(line) {
                    Reply::Answer(answer) => {
                        self.answered.insert(id);
                        self.enter(Input::Free, now_ms);
                        self.advance(now_ms)?;
                        Ok(vec![Action::Answer(id, answer)])
                    }
                    Reply::Next => {
                        let asking = prompt.asking();
                        self.say(&asking)?;
                        Ok(self.stop_timeout(id).into_iter().collect())
                    }
                    Reply::Retry(why) => {
                        let asking = prompt.asking();
                        self.say(&format!("rho: {why}\n{asking}"))?;
                        Ok(self.stop_timeout(id).into_iter().collect())
                    }
                }
            }
        }
    }

    /// Takes the daemon's refusal of the answer to request `id`.
    pub(super) fn refused(
        &mut self,
        id: InputRequestId,
        refusal: Refusal,
        now_ms: u64,
    ) -> io::Result<()> {
        self.answered.remove(&id);
        match refusal {
            Refusal::NotOpen => {
                // Its resolution, if still to come, finds it gone.
                self.open.remove(&id);
                self.held.remove(&id);
                self.tell("could not answer the request: it is no longer open")
            }
            Refusal::Other(why) => {
                self.tell(&format!("could not answer the request: {why}"))?;
                // Still open, so asked again when its turn comes.
                self.advance(now_ms)
            }
        }
    }

    /// Tells the user `notice`, on one line, such as why the daemon refused
    /// a request.
    pub(super) fn tell(&mut self, notice: &str) -> io::Result<()> {
        writeln!(
            self.err,
            "rho: {}",
            escape_controls(notice, LineBreaks::Escape)
        )
    }

    /// No more lines can be read: requests from now on are only told.
    pub(super) fn closed(&mut self) -> io::Result<()> {
        let previous = std::mem::replace(&mut self.input, Input::Closed);
        match previous {
            Input::Answering(prompt) if self.held.contains(&prompt.id()) => {
                self.tell("no more input; another client can answer it, and it no longer times out")
            }
            Input::Answering(_) => self.tell("no more input; another client can answer it"),
            Input::Free | Input::Closed => Ok(()),
        }
    }

    /// `/hold` at the prompt for request `id`.
    fn hold_command(&mut self, id: InputRequestId) -> io::Result<Vec<Action>> {
        let hold = self.stop_timeout(id);
        self.tell(match hold {
            Some(_) => "holding it: the daemon waits for your answer",
            None => "it does not time out",
        })?;
        Ok(hold.into_iter().collect())
    }

    /// The hold that stops request `id`'s timeout, if it has one, as once
    /// the user started on it. The timeout stays until `input_held` says
    /// it stopped, so a hold the daemon refused is asked again and never
    /// counts as held.
    fn stop_timeout(&mut self, id: InputRequestId) -> Option<Action> {
        self.open.get(&id)?.timeout.as_ref()?;
        Some(Action::Hold(id))
    }

    /// Whether request `id` is being answered.
    fn answering(&self, id: InputRequestId) -> bool {
        matches!(&self.input, Input::Answering(prompt) if prompt.id() == id)
    }

    /// Starts `input` at `now_ms`.
    fn enter(&mut self, input: Input, now_ms: u64) {
        self.input = input;
        self.since_ms = now_ms;
    }

    /// Request `request` opened: shown at once when nothing else is being
    /// answered, otherwise told, with when the daemon answers it itself.
    fn arrived(&mut self, request: InputRequest, now_ms: u64) -> io::Result<()> {
        let id = request.id;
        self.open.insert(id, request);
        self.advance(now_ms)?;
        if self.answering(id) {
            return Ok(());
        }
        let request = &self.open[&id];
        let mut text = format!("rho: the turn waits for {}\n", asked(&request.ask));
        if Prompt::new(request).is_none() {
            text.push_str("rho: this rho cannot answer it; answer it from another client\n");
        } else if matches!(self.input, Input::Closed) {
            text.push_str("rho: not reading input; answer it from another client\n");
        } else if let Some(timeout) = &request.timeout {
            text.push_str(&format!(
                "rho: the daemon answers it in {}s unless it is answered first ({})\n",
                seconds_left(timeout.deadline_ms, now_ms),
                one_line(&timeout.reason)
            ));
        }
        self.say(&text)
    }

    /// Request `id` closed, as `how` says: told unless this console
    /// answered it. A line typed for it from then on is dropped.
    fn close(&mut self, id: InputRequestId, how: &str, now_ms: u64) -> io::Result<()> {
        let Some(request) = self.open.remove(&id) else {
            return Ok(());
        };
        self.held.remove(&id);
        if self.answered.remove(&id) {
            return Ok(());
        }
        if self.answering(id) {
            self.enter(Input::Free, now_ms);
        }
        let asked = asked(&request.ask);
        self.say(&format!(
            "rho: the turn no longer waits for {asked} ({how})\n"
        ))
    }

    /// Shows the oldest open request this console can answer when nothing
    /// is being answered.
    fn advance(&mut self, now_ms: u64) -> io::Result<()> {
        if !matches!(self.input, Input::Free) {
            return Ok(());
        }
        let Some(prompt) = self
            .open
            .values()
            .filter(|request| !self.answered.contains(&request.id))
            .find_map(Prompt::new)
        else {
            return Ok(());
        };
        let request = &self.open[&prompt.id()];
        let mut text = format!(
            "rho: the turn waits for {}\n{}",
            asked(&request.ask),
            prompt.opening()
        );
        if let Some(timeout) = &request.timeout {
            text.push_str(&format!(
                "rho: the daemon answers in {}s unless you answer first or type /hold ({})\n",
                seconds_left(timeout.deadline_ms, now_ms),
                one_line(&timeout.reason)
            ));
        }
        self.enter(Input::Answering(prompt), now_ms);
        self.say(&text)
    }

    /// Writes `text`, lines whose request fields are already escaped.
    fn say(&mut self, text: &str) -> io::Result<()> {
        write!(self.err, "{text}")
    }
}

/// `field` of a request, escaped to show on one line.
pub(super) fn one_line(field: &str) -> String {
    escape_controls(field, LineBreaks::Escape)
}

/// Whether `line` reads as a command: a slash and one word.
fn is_command(line: &str) -> bool {
    line.strip_prefix('/')
        .is_some_and(|word| !word.is_empty() && word.chars().all(|ch| ch.is_ascii_alphabetic()))
}

/// Whole seconds from `now_ms` until `deadline_ms`, rounded up.
fn seconds_left(deadline_ms: u64, now_ms: u64) -> u64 {
    deadline_ms.saturating_sub(now_ms).div_ceil(1000)
}

/// What `ask` waits for, for a notice. An approval names its operation in
/// full, since mode policies leave the reason empty for ordinary writes and
/// commands: what the user allows is the operation.
fn asked(ask: &Ask) -> String {
    match ask {
        Ask::Approval(approval) => {
            let mut text = format!("an approval to {}", operation(&approval.operation));
            if !approval.reason.is_empty() {
                text.push_str(&format!(" ({})", one_line(&approval.reason)));
            }
            text
        }
        Ask::PlanApproval(_) => "its plan to be approved".to_owned(),
        Ask::Question(question) => format!("answers to \"{}\"", one_line(&question.title)),
        // `Ask` is non-exhaustive.
        _ => "input".to_owned(),
    }
}

/// What `operation` does, whole, on one line: a command's line breaks are
/// shown as `\n`, so none of it can hide.
fn operation(operation: &Operation) -> String {
    let path = |verb: &str, path: &std::path::Path, scope: &PathScope| {
        let outside = match scope {
            PathScope::Unrestricted => " (outside the workspace)",
            // `PathScope` is non-exhaustive; a granted root is the user's.
            _ => "",
        };
        format!("{verb} {}{outside}", one_line(&path.to_string_lossy()))
    };
    match operation {
        Operation::ReadPath { path: at, scope } => path("read", at, scope),
        Operation::WritePath { path: at, scope } => path("write", at, scope),
        Operation::DiscoverInstructions { path: at, scope } => {
            path("discover instructions in", at, scope)
        }
        Operation::RunProcess {
            program,
            args,
            shell_command,
            cwd,
        } => {
            let command = match shell_command {
                Some(command) => command.clone(),
                None => std::iter::once(program.to_string_lossy().into_owned())
                    .chain(args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            format!(
                "run {} in {}",
                one_line(&command),
                one_line(&cwd.to_string_lossy())
            )
        }
        Operation::Network { url: Some(url) } => format!("access {}", one_line(url)),
        Operation::Network { url: None } => "access the network".to_owned(),
        Operation::LoadSkill { name, .. } => format!("load skill {}", one_line(name)),
        // `Operation` is non-exhaustive.
        _ => "do something this rho cannot name".to_owned(),
    }
}

/// How a request closed without this console's answer.
fn resolved(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Answered => "another client answered it",
        Resolution::TimedOut => "the daemon answered it at its timeout",
        Resolution::Withdrawn => "it was withdrawn",
        // `Resolution` is non-exhaustive.
        _ => "it closed",
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
