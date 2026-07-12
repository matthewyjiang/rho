use std::{
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

use crate::model::Message;
use crate::session::Session;

pub trait HistorySink: Send {
    fn append_message(&mut self, message: &Message) -> anyhow::Result<()>;
    fn replace_history(&mut self, messages: &[Message]) -> anyhow::Result<()>;
}

enum PersistenceCommand {
    Append(Message),
    Replace(Vec<Message>),
}

pub struct SessionHistorySink {
    command_tx: Option<Sender<PersistenceCommand>>,
    worker: Option<JoinHandle<()>>,
}

impl SessionHistorySink {
    pub fn new(session: Session) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("rho-session-persistence".into())
            .spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    // Keep per-entry fsync durability while moving file and SQLite work off Tokio.
                    // Persistence failures are best-effort, matching the index update behavior.
                    let _ = match command {
                        PersistenceCommand::Append(message) => session.append_message(&message),
                        PersistenceCommand::Replace(messages) => session.replace_history(&messages),
                    };
                }
            })
            .expect("session persistence worker should start");
        Self {
            command_tx: Some(command_tx),
            worker: Some(worker),
        }
    }
}

impl HistorySink for SessionHistorySink {
    fn append_message(&mut self, message: &Message) -> anyhow::Result<()> {
        self.command_tx
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session persistence worker stopped"))?
            .send(PersistenceCommand::Append(message.clone()))
            .map_err(|_| anyhow::anyhow!("session persistence worker stopped"))
    }

    fn replace_history(&mut self, messages: &[Message]) -> anyhow::Result<()> {
        self.command_tx
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("session persistence worker stopped"))?
            .send(PersistenceCommand::Replace(messages.to_vec()))
            .map_err(|_| anyhow::anyhow!("session persistence worker stopped"))
    }
}

impl Drop for SessionHistorySink {
    fn drop(&mut self) {
        self.command_tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
