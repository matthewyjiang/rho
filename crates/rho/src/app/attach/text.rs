//! `rho attach`'s text output: the conversation on stdout, as it streams,
//! and notices on stderr. The console tells and answers input requests.

use std::{
    collections::{hash_map::Entry, HashMap},
    io::{self, Write},
};

use rho_protocol::{
    CompactionStatus, ContentBlock, SessionEvent, ToolCallId, ToolCallUpdate, ToolStatus,
};

use crate::control_chars::{escape_controls, LineBreaks};

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
                        // A saved user message holds only text and images;
                        // `ContentBlock` is non-exhaustive.
                        _ => "[content]",
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                self.block(&quoted(&text))?;
            }
            SessionEvent::AgentMessageChunk { text, .. } => self.write(text)?,
            SessionEvent::ToolCall(update) => self.tool_call(update)?,
            // A terminal shows text as is, so the card's plain form.
            SessionEvent::Card { title, plain, .. } => {
                self.block(&format!("{title}\n{}\n", plain.trim_end_matches('\n')))?;
            }
            SessionEvent::Compaction { status, .. } => self.compaction(status)?,
            SessionEvent::Notice { text } => {
                writeln!(self.err, "rho: {}", escape_controls(text, LineBreaks::Keep))?
            }
            SessionEvent::ResponseDiscarded => {
                writeln!(self.err, "rho: provider response discarded; retrying")?
            }
            SessionEvent::TurnEnded { .. } => self.end_line()?,
            SessionEvent::ThoughtChunk { .. }
            | SessionEvent::Plan { .. }
            | SessionEvent::Usage { .. }
            | SessionEvent::TitleChanged { .. }
            | SessionEvent::State { .. }
            | SessionEvent::InputRequested { .. }
            | SessionEvent::InputResolved { .. }
            | SessionEvent::InputHeld { .. }
            | SessionEvent::OptionsChanged { .. }
            | SessionEvent::ProviderRequest
            // A retry shows only as the response it discarded, if any.
            | SessionEvent::ProviderRetry { .. }
            // The steer shows once it applies, as a user message.
            | SessionEvent::SteeringDelivered { .. }
            | SessionEvent::Background { .. } => {}
            // `SessionEvent` is non-exhaustive; an event a newer daemon sends
            // is not shown.
            _ => {}
        }
        self.out.flush()
    }

    /// Shows a tool call once it has a title, and again once it fails.
    fn tool_call(&mut self, update: &ToolCallUpdate) -> io::Result<()> {
        // A call the model is still writing has no arguments to title it
        // with; it shows once it starts.
        if update.status == Some(ToolStatus::Pending) {
            return Ok(());
        }
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

    /// Shows a compaction once it starts, and again if it fails; a cancel
    /// shows as the turn's end.
    fn compaction(&mut self, status: &CompactionStatus) -> io::Result<()> {
        match status {
            CompactionStatus::Started { .. } => self.block("• compact context\n"),
            CompactionStatus::Failed { .. } => self.block("• compact context failed\n"),
            CompactionStatus::Completed { .. } | CompactionStatus::Cancelled => Ok(()),
            // `CompactionStatus` is non-exhaustive.
            _ => Ok(()),
        }
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
        // The conversation is the model's: escaped, so it cannot steer the
        // terminal, such as hide the requests the console shows after it.
        self.out
            .write_all(escape_controls(text, LineBreaks::Keep).as_bytes())?;
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

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
