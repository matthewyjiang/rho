//! The prompts a session knows by their idempotency key (#1777), which a
//! prompt sent again with the key answers as, starting nothing.
//!
//! A keyed prompt's user message is saved under a [`MessageKey`] that names
//! a one-way digest of the session and the key, then the prompt's turn
//! ([`row_key`]). A load finds every keyed prompt in the session's saved
//! rows, so the daemon knows a key as long as its message is saved, across
//! restarts too, and only the digest is ever saved or shown. A prompt that
//! saves no message, such as `/compact`, is known while the daemon keeps the
//! session loaded, and, as the job records the latest turn's digest and
//! message, until another turn starts.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use rho_protocol::{IdempotencyKey, MessageId, TurnId};
use rho_sdk::SessionId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::{
    super::jobs::{JobRecord, TurnRecord},
    PromptStart,
};
use crate::{
    app::session_events::user_message_id,
    session::tree::{MessageKey, ProjectedMessage},
};

/// A prompt's idempotency key as the session keeps it: a one-way digest of
/// the session's id and the key, so a key names nothing in another session
/// and a saved row never reveals it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(in crate::app::daemon_host) struct KeyDigest(String);

/// How many hex digits of the SHA-256 a [`KeyDigest`] keeps: 128 bits, as
/// many as the UUID the ACP spec suggests clients use for the key.
const DIGEST_HEX: usize = 32;

/// What a [`row_key`] starts with.
const ROW_KEY_PREFIX: &str = "prompt-";

impl KeyDigest {
    pub(super) fn of(session: &SessionId, key: &IdempotencyKey) -> Self {
        let session = session.as_str();
        let mut hash = Sha256::new();
        // Length-prefixed, so no other session and key hash the same bytes.
        hash.update((session.len() as u64).to_be_bytes());
        hash.update(session);
        hash.update(key.as_str());
        let mut hex = hex::encode(hash.finalize());
        hex.truncate(DIGEST_HEX);
        Self(hex)
    }
}

/// The key the user message of turn `turn`, whose prompt named `digest`, is
/// saved under. A replay names the message from it, as live does.
pub(super) fn row_key(digest: &KeyDigest, turn: &TurnId) -> MessageKey {
    MessageKey::new(format!("{ROW_KEY_PREFIX}{}-{turn}", digest.0))
}

/// The digest and turn a saved row's key names, when a keyed prompt's user
/// message was saved under it.
fn parse_row_key(key: &MessageKey) -> Option<(KeyDigest, TurnId)> {
    let rest = key.as_str().strip_prefix(ROW_KEY_PREFIX)?;
    let digest = rest.get(..DIGEST_HEX)?;
    let turn = rest.get(DIGEST_HEX..)?.strip_prefix('-')?;
    let hex = digest.bytes().all(|byte| byte.is_ascii_hexdigit());
    (hex && !turn.is_empty()).then(|| (KeyDigest(digest.to_owned()), TurnId::new(turn)))
}

/// The user message a prompt's turn inserted, set before the turn tells its
/// prompt it began ([`PromptStart::Began`]), so the request, and one that
/// sends the prompt again, answer with it. Unset for a turn that inserted
/// none, or that began before it did, queued for capacity.
#[derive(Clone, Debug, Default)]
pub(super) struct PromptMessage(Arc<OnceLock<MessageId>>);

impl PromptMessage {
    fn known(message: MessageId) -> Self {
        Self(Arc::new(OnceLock::from(message)))
    }

    /// Records the turn's user message; only its first counts.
    pub(super) fn set(&self, message: MessageId) {
        let _ = self.0.set(message);
    }

    pub(super) fn get(&self) -> Option<MessageId> {
        self.0.get().cloned()
    }
}

/// The turn an earlier prompt naming a key started, as a prompt sent again
/// with the key answers.
#[derive(Clone)]
pub(super) struct SentBefore {
    pub(super) id: TurnId,
    /// How the turn began, once it has.
    pub(super) start: watch::Receiver<PromptStart>,
    pub(super) message: PromptMessage,
}

impl SentBefore {
    /// A turn that began, inserting `message` if it is known.
    fn began(id: TurnId, message: Option<MessageId>) -> Self {
        // The sender is dropped: the turn began, and nothing changes that.
        let (_, start) = watch::channel(PromptStart::Began);
        let message = message.map_or_else(PromptMessage::default, PromptMessage::known);
        Self { id, start, message }
    }
}

/// The keyed prompts a session knows, by their digest, which its turn lock
/// guards.
pub(super) struct PromptKeys(HashMap<KeyDigest, SentBefore>);

impl PromptKeys {
    /// The keyed prompts a session's load finds: those its saved `rows`
    /// name, and the turn its job `job` last ran, as an earlier daemon or
    /// an earlier load recorded it, if a prompt naming a key started it and
    /// it began. One that did not begin ran nothing, so its key starts a
    /// turn again.
    pub(super) fn loaded(job: &JobRecord, rows: &[ProjectedMessage]) -> Self {
        let mut keys: HashMap<_, _> = rows
            .iter()
            .filter_map(|row| {
                let key = row.key.as_ref()?;
                let (digest, turn) = parse_row_key(key)?;
                Some((digest, SentBefore::began(turn, Some(user_message_id(key)))))
            })
            .collect();
        if let Some(TurnRecord {
            id,
            key_digest: Some(digest),
            began: true,
            message,
            continues_after_restart: _,
        }) = job.turn.clone()
        {
            // A saved row names the turn's message as well as the record.
            keys.entry(digest)
                .or_insert_with(|| SentBefore::began(id, message));
        }
        Self(keys)
    }

    /// The turn an earlier prompt naming `digest` started, if that turn
    /// began or may yet. A turn that ended before it began ran nothing: its
    /// key starts a turn again.
    pub(super) fn sent_before(&self, digest: &KeyDigest) -> Option<SentBefore> {
        self.0
            .get(digest)
            .filter(|sent| !matches!(*sent.start.borrow(), PromptStart::Ended(_)))
            .cloned()
    }

    /// The prompt naming `digest` started the turn `sent`.
    pub(super) fn turn_started(&mut self, digest: KeyDigest, sent: SentBefore) {
        self.0.insert(digest, sent);
    }
}
