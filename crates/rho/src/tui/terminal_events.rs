use std::io;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;

#[cfg(windows)]
#[path = "windows_input.rs"]
mod windows;
#[cfg(windows)]
pub(super) use windows::PasteMode;

#[cfg(any(windows, test))]
#[path = "windows_input_parser.rs"]
mod parser;
#[cfg(all(test, not(windows)))]
#[path = "windows_input_records.rs"]
mod windows_records;

pub(super) struct TerminalEvents {
    #[cfg(not(windows))]
    stream: EventStream,
    #[cfg(windows)]
    stream: Option<WindowsStream>,
}

#[cfg(windows)]
enum WindowsStream {
    Vt(windows::Input),
    Legacy(EventStream),
}

impl TerminalEvents {
    pub(super) fn new() -> Self {
        Self {
            #[cfg(not(windows))]
            stream: EventStream::new(),
            #[cfg(windows)]
            stream: None,
        }
    }

    pub(super) async fn next(&mut self) -> io::Result<Event> {
        #[cfg(not(windows))]
        {
            event_result(self.stream.next().await)
        }
        #[cfg(windows)]
        {
            // Keyboard modes may be acquired after this object is constructed.
            // Do not start either console reader until the first request.
            if self.stream.is_none() {
                self.stream = Some(match windows::Input::start()? {
                    Some(input) => WindowsStream::Vt(input),
                    None => WindowsStream::Legacy(EventStream::new()),
                });
            }
            match self.stream.as_mut().expect("initialized above") {
                WindowsStream::Vt(input) => input.next().await,
                WindowsStream::Legacy(stream) => event_result(stream.next().await),
            }
        }
    }
}

fn event_result(event: Option<io::Result<Event>>) -> io::Result<Event> {
    event.unwrap_or_else(|| Err(stream_ended_error()))
}

fn stream_ended_error() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream ended")
}
