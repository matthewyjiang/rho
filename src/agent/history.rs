use crate::model::Message;
use crate::session::Session;

pub trait HistorySink: Send {
    fn append_message(&mut self, message: &Message) -> anyhow::Result<()>;
    fn replace_history(&mut self, messages: &[Message]) -> anyhow::Result<()>;
}

#[derive(Clone, Debug)]
pub struct SessionHistorySink {
    session: Session,
}

impl SessionHistorySink {
    pub fn new(session: Session) -> Self {
        Self { session }
    }
}

impl HistorySink for SessionHistorySink {
    fn append_message(&mut self, message: &Message) -> anyhow::Result<()> {
        self.session.append_message(message)
    }

    fn replace_history(&mut self, messages: &[Message]) -> anyhow::Result<()> {
        self.session.replace_history(messages)
    }
}
