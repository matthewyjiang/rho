//! `rho attach`'s text output: the conversation on stdout, as it streams,
//! and what is not part of it, such as notices and input requests, on
//! stderr.

use std::{
    collections::{hash_map::Entry, HashMap},
    io::{self, Write},
};

use rho_protocol::{Ask, ContentBlock, SessionEvent, ToolCallId, ToolCallUpdate, ToolStatus};

/// Writes one session's events for a person reading a terminal.
pub(super) struct TextView<O = io::Stdout, E = io::Stderr> {
    out: O,
    err: E,
    /// The tool calls shown, by id.
    calls: HashMap<ToolCallId, ShownCall>,
    /// Whether `out` ends a line, so the next block starts on its own.
    line_ended: bool,
}

impl TextView {
    pub(super) fn terminal() -> Self {
        Self::new(io::stdout(), io::stderr())
    }
}

impl<O: Write, E: Write> TextView<O, E> {
    pub(super) fn new(out: O, err: E) -> Self {
        Self {
            out,
            err,
            calls: HashMap::new(),
            line_ended: true,
        }
    }

    /// Writes `event`, if a person reading the conversation needs it.
    pub(super) fn show(&mut self, event: &SessionEvent) -> io::Result<()> {
        match event {
            SessionEvent::UserMessage { content, .. } => {
                let text = content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => text.as_str(),
                        ContentBlock::Image { .. } => "[image]",
                        // `ContentBlock` is non-exhaustive.
                        _ => "[content]",
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.block(&quoted(&text))?;
            }
            SessionEvent::AgentMessageChunk { text, .. } => self.write(text)?,
            SessionEvent::ToolCall(update) => self.tool_call(update)?,
            SessionEvent::Card { title, markdown } => {
                self.block(&format!("{title}\n{}\n", markdown.trim_end_matches('\n')))?;
            }
            SessionEvent::Notice { text } => writeln!(self.err, "rho: {text}")?,
            SessionEvent::InputRequested { request } => {
                writeln!(self.err, "rho: the turn waits for {}", asked(&request.ask))?;
            }
            SessionEvent::TurnEnded { .. } => self.end_line()?,
            SessionEvent::ThoughtChunk { .. }
            | SessionEvent::Plan { .. }
            | SessionEvent::Usage { .. }
            | SessionEvent::State { .. }
            | SessionEvent::InputResolved { .. }
            | SessionEvent::InputHeld { .. } => {}
            // `SessionEvent` is non-exhaustive; an event a newer daemon sends
            // is not shown.
            _ => {}
        }
        self.out.flush()
    }

    /// Shows a tool call once it has a title, and again once it fails.
    fn tool_call(&mut self, update: &ToolCallUpdate) -> io::Result<()> {
        let mut lines = String::new();
        let call = match (self.calls.entry(update.id.clone()), &update.title) {
            (Entry::Occupied(entry), title) => {
                let call = entry.into_mut();
                if let Some(title) = title {
                    call.title.clone_from(title);
                }
                call
            }
            (Entry::Vacant(entry), Some(title)) => {
                lines.push_str(&format!("• {title}\n"));
                entry.insert(ShownCall {
                    title: title.clone(),
                    failed: false,
                })
            }
            // Not shown until it has a title.
            (Entry::Vacant(_), None) => return Ok(()),
        };
        if update.status == Some(ToolStatus::Failed) && !call.failed {
            call.failed = true;
            lines.push_str(&format!("• {} failed\n", call.title));
        }
        if lines.is_empty() {
            return Ok(());
        }
        self.block(&lines)
    }

    /// Writes `text` on lines of its own.
    fn block(&mut self, text: &str) -> io::Result<()> {
        self.end_line()?;
        self.write(text)
    }

    /// Ends the line the conversation stopped on, so what follows on the
    /// terminal, such as why the attachment ended, starts a line of its own.
    pub(super) fn finish(&mut self) -> io::Result<()> {
        self.end_line()?;
        self.out.flush()
    }

    /// Starts the conversation over, as a reset replay writes it again from
    /// the start: tool calls shown before are shown again.
    pub(super) fn restart(&mut self) -> io::Result<()> {
        self.calls.clear();
        self.end_line()
    }

    fn end_line(&mut self) -> io::Result<()> {
        if self.line_ended {
            return Ok(());
        }
        self.write("\n")
    }

    fn write(&mut self, text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.out.write_all(text.as_bytes())?;
        self.line_ended = text.ends_with('\n');
        Ok(())
    }
}

/// A tool call the view showed.
struct ShownCall {
    title: String,
    /// Whether its failure was shown.
    failed: bool,
}

/// `text` as a quoted block: each line starts with `> `.
fn quoted(text: &str) -> String {
    text.lines().map(|line| format!("> {line}\n")).collect()
}

/// What `ask` waits for, for a notice.
fn asked(ask: &Ask) -> String {
    match ask {
        Ask::Approval(approval) => format!("an approval: {}", approval.reason),
        Ask::PlanApproval(_) => "its plan to be approved".to_owned(),
        Ask::Question(question) => format!("answers to \"{}\"", question.title),
        // `Ask` is non-exhaustive.
        _ => "input".to_owned(),
    }
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
