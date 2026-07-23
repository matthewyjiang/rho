use std::io;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;

pub(super) struct TerminalEvents {
    stream: EventStream,
}

impl TerminalEvents {
    pub(super) fn new() -> Self {
        Self {
            stream: EventStream::new(),
        }
    }

    pub(super) async fn next(&mut self) -> io::Result<Event> {
        event_result(self.stream.next().await)
    }
}

fn event_result(event: Option<io::Result<Event>>) -> io::Result<Event> {
    event.unwrap_or_else(|| Err(stream_ended_error()))
}

fn stream_ended_error() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream ended")
}
