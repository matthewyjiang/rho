//! Display rows as stored on session tree nodes.
//!
//! A node stores the model messages it adds (`transition`) and the transcript
//! rows it shows (`display_messages`). The two are usually the same messages.
//! Version 5 files store such a row as a reference, `{"model":3}`: "message 3
//! of this node's own transition". Only rows that differ from every message
//! the node stores keep full content, `{"message":{...}}`; user input rewritten
//! for the model and background results are the usual cases.
//!
//! References never leave their node, so readers resolve them from one record:
//! the tree at insert time, and search evidence from the raw JSON record.
//! Version 1-4 rows always carry `timestamp` and `message`; older binaries
//! reject version 5 headers, so they never see a reference.

use rho_providers::model::Message;
use serde::{Deserialize, Serialize, Serializer};

use super::persistence::StoredDisplayMessage;

/// First session version whose node display rows may reference model messages.
pub(super) const MODEL_REF_VERSION: u32 = 5;

/// How a session file stores display rows; fixed by its header version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayRowFormat {
    /// Versions 1-4: every row repeats its timestamp and full message.
    Inline,
    /// Version 5: rows identical to a message of the same node reference it.
    ModelRefs,
}

impl DisplayRowFormat {
    pub(super) fn for_version(version: u32) -> Self {
        if version >= MODEL_REF_VERSION {
            Self::ModelRefs
        } else {
            Self::Inline
        }
    }
}

/// One stored display row.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "RawDisplayRow<String, Message>")]
pub(crate) enum StoredDisplayRow {
    /// The message at this index of the node's own model messages (the
    /// appended history of a delta, the full history of a snapshot), shown
    /// at the node timestamp.
    Model(usize),
    /// Full content. A `None` timestamp means the node timestamp.
    Inline {
        timestamp: Option<String>,
        message: Message,
    },
}

impl StoredDisplayRow {
    /// Stores `tail` for a node whose transition holds `model`, at `timestamp`.
    ///
    /// Rows normally follow model order, so the search starts after the last
    /// match and wraps once; a repeated message may resolve to any equal copy.
    pub(crate) fn encode(
        tail: &[Message],
        model: &[Message],
        format: DisplayRowFormat,
        timestamp: &str,
    ) -> Vec<Self> {
        if format == DisplayRowFormat::Inline {
            return tail
                .iter()
                .map(|message| Self::Inline {
                    timestamp: Some(timestamp.to_owned()),
                    message: message.clone(),
                })
                .collect();
        }
        let mut next = 0;
        tail.iter()
            .map(|message| {
                let found = model[next..]
                    .iter()
                    .position(|candidate| candidate == message)
                    .map(|offset| next + offset)
                    .or_else(|| {
                        model[..next]
                            .iter()
                            .position(|candidate| candidate == message)
                    });
                match found {
                    Some(index) => {
                        next = index + 1;
                        Self::Model(index)
                    }
                    None => Self::Inline {
                        timestamp: None,
                        message: message.clone(),
                    },
                }
            })
            .collect()
    }

    /// Resolves stored rows into full display messages. Errors name the
    /// node so a malformed or mis-versioned record fails the load loudly.
    pub(crate) fn resolve(
        rows: Vec<Self>,
        model: &[Message],
        format: DisplayRowFormat,
        timestamp: &str,
        node_id: &str,
    ) -> anyhow::Result<Vec<StoredDisplayMessage>> {
        rows.into_iter()
            .map(|row| match (row, format) {
                (
                    Self::Inline {
                        timestamp: Some(timestamp),
                        message,
                    },
                    _,
                ) => Ok(StoredDisplayMessage { timestamp, message }),
                (
                    Self::Inline {
                        timestamp: None,
                        message,
                    },
                    DisplayRowFormat::ModelRefs,
                ) => Ok(StoredDisplayMessage {
                    timestamp: timestamp.to_owned(),
                    message,
                }),
                (Self::Model(index), DisplayRowFormat::ModelRefs) => {
                    let message = model.get(index).ok_or_else(|| {
                        anyhow::anyhow!(
                            "node '{node_id}' display row references model message {index}, \
                             but the node stores {}",
                            model.len()
                        )
                    })?;
                    Ok(StoredDisplayMessage {
                        timestamp: timestamp.to_owned(),
                        message: message.clone(),
                    })
                }
                (
                    Self::Inline {
                        timestamp: None, ..
                    }
                    | Self::Model(_),
                    DisplayRowFormat::Inline,
                ) => {
                    anyhow::bail!(
                        "node '{node_id}' display row needs a timestamp and message before \
                         session version {MODEL_REF_VERSION}"
                    )
                }
            })
            .collect()
    }
}

/// JSON shape shared by both variants; exactly one of `model` and `message`.
/// Generic so saves serialize borrowed content. Missing `Option` fields
/// deserialize as `None`.
#[derive(Deserialize, Serialize)]
struct RawDisplayRow<T, M> {
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamp: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<M>,
}

impl TryFrom<RawDisplayRow<String, Message>> for StoredDisplayRow {
    type Error = String;

    fn try_from(row: RawDisplayRow<String, Message>) -> Result<Self, Self::Error> {
        match row {
            RawDisplayRow {
                timestamp: None,
                model: Some(index),
                message: None,
            } => Ok(Self::Model(index)),
            RawDisplayRow {
                timestamp,
                model: None,
                message: Some(message),
            } => Ok(Self::Inline { timestamp, message }),
            _ => Err(
                "display row needs exactly one of `model` or `message`, and model rows take no timestamp"
                    .into(),
            ),
        }
    }
}

impl Serialize for StoredDisplayRow {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Model(index) => RawDisplayRow::<&str, &Message> {
                timestamp: None,
                model: Some(*index),
                message: None,
            },
            Self::Inline { timestamp, message } => RawDisplayRow {
                timestamp: timestamp.as_deref(),
                model: None,
                message: Some(message),
            },
        }
        .serialize(serializer)
    }
}

#[cfg(test)]
#[path = "display_rows_tests.rs"]
mod tests;
