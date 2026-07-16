use std::io;

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use tokio::{sync::mpsc, task::JoinHandle};

const EVENT_BUFFER_CAPACITY: usize = 256;

pub(super) struct TerminalEvents {
    receiver: mpsc::Receiver<io::Result<Event>>,
    reader: JoinHandle<()>,
}

impl TerminalEvents {
    pub(super) fn new() -> Self {
        let (sender, receiver) = mpsc::channel(EVENT_BUFFER_CAPACITY);
        let reader = tokio::spawn(read_events(sender));
        Self { receiver, reader }
    }

    pub(super) async fn next(&mut self) -> io::Result<Event> {
        event_result(self.receiver.recv().await)
    }

    pub(super) fn try_next(&mut self) -> Option<io::Result<Event>> {
        match self.receiver.try_recv() {
            Ok(event) => Some(event),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => Some(Err(stream_ended_error())),
        }
    }
}

impl Drop for TerminalEvents {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn read_events(sender: mpsc::Sender<io::Result<Event>>) {
    let mut stream = EventStream::new();
    while let Some(event) = stream.next().await {
        let stream_failed = event.is_err();
        if sender.send(event).await.is_err() || stream_failed {
            return;
        }
    }
}

fn event_result(event: Option<io::Result<Event>>) -> io::Result<Event> {
    event.unwrap_or_else(|| Err(stream_ended_error()))
}

fn stream_ended_error() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "terminal event stream ended")
}
