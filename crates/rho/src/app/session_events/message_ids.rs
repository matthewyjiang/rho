//! The ids messages are shown under, live and replayed alike.
//!
//! A turn's saves key the rows of its model replies and steers
//! ([`TurnKeys`]). Live, [`MessageStream`] names those messages from the
//! same keys as they stream, and replay names a keyed row's messages from
//! its key ([`keyed_message_id`]), so a message keeps one id whether a
//! client watched it live, got it replayed, or found it with
//! `rho/session/search`. Rows saved without a key, as before keys or by
//! host commands, replay under ids derived from where they were saved.
//!
//! Live text is split into messages at tool calls, as replay splits a
//! saved reply's text, which holds the text between calls as the provider
//! streamed it. Within one stretch, an id continues after an event that
//! interrupts its chunks (a hosted tool's card, a late steer), as replay
//! shows the stretch as one message.

use rho_protocol::MessageId;

use crate::{app::session_runtime::TurnKeys, session::tree::MessageKey};

/// Which of a keyed row's messages an id names. A model reply shows as its
/// reasoning, then a message per stretch of text between its tool calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyedPart {
    /// The row's one message, as a steer's.
    Whole,
    Thought,
    /// The `n`th stretch of text, from 0.
    Agent(u32),
}

/// The id of the message `part` of the row keyed `key`.
pub(super) fn keyed_message_id(key: &MessageKey, part: KeyedPart) -> MessageId {
    let key = key.as_str();
    MessageId::new(match part {
        KeyedPart::Whole => format!("msg-{key}"),
        KeyedPart::Thought => format!("msg-{key}-thought"),
        KeyedPart::Agent(n) => format!("msg-{key}-agent-{n}"),
    })
}

/// The id of the user message saved as the row keyed `key`, live and
/// replayed alike, as a keyed prompt's is.
pub(crate) fn user_message_id(key: &MessageKey) -> MessageId {
    keyed_message_id(key, KeyedPart::Whole)
}

/// The id of a live message no save keys, as one outside a run.
pub(super) fn fresh_message_id() -> MessageId {
    MessageId::new(format!("msg-{}", uuid::Uuid::new_v4()))
}

/// Which message a live chunk streams into.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum ChunkKind {
    Agent,
    Thought,
}

/// Names the messages a session's live chunks and steers stream into: in a
/// run, as its saves key them; outside one, with fresh ids.
#[derive(Default)]
pub(super) struct MessageStream {
    /// The message the latest chunk belongs to, while later chunks of its
    /// kind still continue it.
    open: Option<(ChunkKind, MessageId)>,
    /// The run's keys; `None` outside a run.
    keys: Option<TurnKeys>,
    /// How many of the run's steps started: the reply under way is this one.
    steps: u32,
    /// Whether the step discarded a response that showed something. A
    /// client may keep what it showed, so the retries stream under fresh
    /// ids. The saved reply, the last attempt, replays under the ids the
    /// discarded response streamed under: no save knows of the discard.
    discarded: bool,
    /// The stretch of the reply's text, between its tool calls, that the
    /// next agent chunk belongs to, from 0.
    stretch: u32,
    stretch_state: Stretch,
    /// How many steers applied.
    steers: u32,
}

/// Where the reply's current stretch of text stands.
#[derive(Clone, Copy, Default, PartialEq)]
enum Stretch {
    /// No text yet.
    #[default]
    Empty,
    Open,
    /// A tool call came after its text: the next chunk starts a stretch.
    Closed,
}

impl MessageStream {
    pub(super) fn run_started(&mut self, keys: TurnKeys) {
        *self = Self {
            keys: Some(keys),
            ..Self::default()
        };
    }

    pub(super) fn run_ended(&mut self) {
        self.keys = None;
    }

    /// Something other than a chunk was shown, so the next chunk starts a
    /// message.
    pub(super) fn interrupt(&mut self) {
        self.open = None;
    }

    /// A step started: what streams next is its reply.
    pub(super) fn step_started(&mut self) {
        self.steps += 1;
        self.discarded = false;
        self.new_response();
    }

    /// The step's response was abandoned and is retried, in new messages.
    /// `discarded` says whether it showed clients anything.
    pub(super) fn response_reset(&mut self, discarded: bool) {
        self.discarded |= discarded;
        self.new_response();
    }

    fn new_response(&mut self) {
        self.open = None;
        self.stretch = 0;
        self.stretch_state = Stretch::Empty;
    }

    /// A tool call streamed or started, which ends the stretch of text
    /// before it, as replay splits a reply's text at its calls, whether or
    /// not clients were shown anything of the call yet.
    pub(super) fn call_seen(&mut self) {
        if self.stretch_state == Stretch::Open {
            self.stretch_state = Stretch::Closed;
            if matches!(self.open, Some((ChunkKind::Agent, _))) {
                self.open = None;
            }
        }
    }

    /// The id of the message a chunk of `kind` streams into.
    pub(super) fn chunk(&mut self, kind: ChunkKind) -> MessageId {
        if let Some((open, id)) = &self.open {
            if *open == kind {
                return id.clone();
            }
        }
        let part = match kind {
            ChunkKind::Thought => KeyedPart::Thought,
            ChunkKind::Agent => {
                if self.stretch_state == Stretch::Closed {
                    self.stretch += 1;
                }
                self.stretch_state = Stretch::Open;
                KeyedPart::Agent(self.stretch)
            }
        };
        let id = self
            .reply()
            .map_or_else(fresh_message_id, |reply| keyed_message_id(&reply, part));
        self.open = Some((kind, id.clone()));
        id
    }

    /// The id of the next steer to apply.
    pub(super) fn steer(&mut self) -> MessageId {
        self.steers += 1;
        self.keys.as_ref().map_or_else(fresh_message_id, |keys| {
            keyed_message_id(&keys.steer(self.steers), KeyedPart::Whole)
        })
    }

    /// The key of the reply under way, when its saves key it as shown.
    fn reply(&self) -> Option<MessageKey> {
        let keys = self.keys.as_ref()?;
        (self.steps > 0 && !self.discarded).then(|| keys.reply(self.steps))
    }
}
