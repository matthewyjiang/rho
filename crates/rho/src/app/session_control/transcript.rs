//! A session's events as plain text, for `read`: who said what, which tools
//! ran, and how turns that did not answer ended. Reasoning, usage, and
//! display cards are left out; a reader who needs them attaches.

use std::collections::HashMap;

use rho_protocol::{ContentBlock, MessageId, SessionEvent, ToolCallId, ToolStatus, TurnOutcome};

/// The text a session's events build, block by block.
#[derive(Default)]
pub(super) struct Transcript {
    blocks: Vec<Block>,
    /// Where each tool call's block is, so its updates land on it.
    tools: HashMap<ToolCallId, usize>,
}

enum Block {
    User(String),
    Agent {
        message: MessageId,
        text: String,
    },
    Tool {
        title: String,
        status: Option<ToolStatus>,
    },
    Note(String),
}

impl Transcript {
    pub(super) fn push(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::UserMessage { content, .. } => {
                self.blocks.push(Block::User(content_text(content)));
            }
            SessionEvent::AgentMessageChunk { message, text } => match self.blocks.last_mut() {
                Some(Block::Agent {
                    message: last,
                    text: so_far,
                }) if last == message => so_far.push_str(text),
                _ => self.blocks.push(Block::Agent {
                    message: message.clone(),
                    text: text.clone(),
                }),
            },
            SessionEvent::ToolCall(update) => {
                let at = match self.tools.get(&update.id) {
                    Some(&at) => at,
                    None => {
                        self.blocks.push(Block::Tool {
                            title: update.id.to_string(),
                            status: None,
                        });
                        self.tools.insert(update.id.clone(), self.blocks.len() - 1);
                        self.blocks.len() - 1
                    }
                };
                if let Block::Tool { title, status } = &mut self.blocks[at] {
                    if let Some(new) = &update.title {
                        title.clone_from(new);
                    }
                    if update.status.is_some() {
                        *status = update.status;
                    }
                }
            }
            SessionEvent::InputRequested { request } => self.blocks.push(Block::Note(format!(
                "waiting for a person to answer input request {}",
                request.id
            ))),
            SessionEvent::TurnEnded { outcome } => {
                if let Some(note) = unanswered(outcome) {
                    self.blocks.push(Block::Note(note));
                }
            }
            SessionEvent::ResponseDiscarded => self.blocks.push(Block::Note(
                "the provider discarded the response; retrying".into(),
            )),
            _ => {}
        }
    }

    /// The whole transcript.
    pub(super) fn text(&self) -> String {
        let blocks: Vec<String> = self
            .blocks
            .iter()
            .map(|block| match block {
                Block::User(text) => format!("[user]\n{text}"),
                Block::Agent { text, .. } => format!("[assistant]\n{text}"),
                Block::Tool { title, status } => match status {
                    Some(status) => format!("[tool] {title} ({})", status_name(*status)),
                    None => format!("[tool] {title}"),
                },
                Block::Note(text) => format!("[{text}]"),
            })
            .collect();
        blocks.join("\n\n")
    }
}

/// A user message's text: its text blocks, and a placeholder for others.
fn content_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.clone(),
            _ => "[attachment]".to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How a turn that did not answer ended; `None` for an answer, whose text
/// the transcript already holds.
fn unanswered(outcome: &TurnOutcome) -> Option<String> {
    match outcome {
        TurnOutcome::Answered { .. } => None,
        TurnOutcome::MaxSteps { .. } => Some("the turn ran out of steps".into()),
        TurnOutcome::Cancelled => Some("the turn was cancelled".into()),
        TurnOutcome::TimedOut { timeout_ms } => {
            Some(format!("the turn timed out after {timeout_ms} ms"))
        }
        TurnOutcome::Failed { error } => Some(format!("the turn failed: {}", error.message)),
        _ => Some("the turn ended".into()),
    }
}

fn status_name(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Pending => "pending",
        ToolStatus::InProgress => "running",
        ToolStatus::Completed => "completed",
        ToolStatus::Failed => "failed",
        _ => "unknown",
    }
}

#[cfg(test)]
#[path = "transcript_tests.rs"]
mod tests;
