//! Eval batches: requests the agent made at once, from labeled fixtures or
//! replayed from saved sessions.
//!
//! A batch's history ends with one assistant message holding every member's
//! call, all unanswered, as when parallel calls ask for approval together.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use rho_providers::model::{ContentBlock, Message, ToolCall};
use rho_sdk::ApprovalRequest;
use serde::Deserialize;

use super::cases::{
    replay_request, spread_evenly, CaseSource, Decision, FixtureEntry, FixturePending,
    FIXTURE_WORKSPACE,
};
use crate::{
    history_message::HistoryMessage,
    session::replay_points::{self, HistorySegment},
};

/// Requests made at once, with the history they share.
#[derive(Debug)]
pub(super) struct EvalBatch {
    pub id: String,
    pub source: CaseSource,
    pub category: Option<String>,
    pub workspace: PathBuf,
    pub history: Vec<Message>,
    pub members: Vec<EvalMember>,
}

/// One request of an [`EvalBatch`].
#[derive(Debug)]
pub(super) struct EvalMember {
    /// Unique within its batch.
    pub id: String,
    /// Expected decision. Replayed members are unlabeled.
    pub label: Option<Decision>,
    pub pending: ApprovalRequest,
    /// ID of the member's call in the batch's last history message.
    pub call_id: String,
    /// The pending command or path, for reading reports.
    pub summary: String,
}

/// Loads a JSON Lines batch fixture file. Blank lines are skipped; batch IDs
/// must be unique, and member IDs unique within their batch.
pub(super) fn load_batch_file(path: &Path) -> anyhow::Result<Vec<EvalBatch>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    parse_batches(&text).with_context(|| format!("invalid batch file {}", path.display()))
}

pub(super) fn parse_batches(text: &str) -> anyhow::Result<Vec<EvalBatch>> {
    let mut ids = HashSet::new();
    let mut batches = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_number = index + 1;
        let batch: FixtureBatch =
            serde_json::from_str(line).with_context(|| format!("line {line_number}"))?;
        anyhow::ensure!(
            ids.insert(batch.id.clone()),
            "line {line_number}: duplicate batch id {}",
            batch.id
        );
        anyhow::ensure!(
            batch.pending.len() >= 2,
            "line {line_number}: batch {} needs at least 2 pending requests",
            batch.id
        );
        let mut member_ids = HashSet::new();
        for member in &batch.pending {
            anyhow::ensure!(
                member_ids.insert(member.id.as_str()),
                "line {line_number}: duplicate member id {} in batch {}",
                member.id,
                batch.id
            );
        }
        batches.push(batch.into_eval_batch());
    }
    Ok(batches)
}

/// Replays up to `per_session` sibling groups from a saved session, spread
/// evenly through its whole active path.
pub(super) fn replay_session(path: &Path, per_session: usize) -> anyhow::Result<Vec<EvalBatch>> {
    let (session_id, segments) = replay_points::segments(path)?;
    let cwd = replay_points::session_cwd(path)?;
    Ok(replay_batches(&session_id, &cwd, &segments, per_session))
}

/// A sibling group is an assistant message with at least two calls that can
/// reach the classifier (see [`replay_request`]); its batch history ends with
/// that message.
pub(super) fn replay_batches(
    session_id: &str,
    cwd: &Path,
    segments: &[HistorySegment],
    per_session: usize,
) -> Vec<EvalBatch> {
    let mut groups = Vec::new();
    for (segment_index, segment) in segments.iter().enumerate() {
        let history = &segment.messages;
        for (index, message) in history.iter().enumerate().skip(segment.new_from) {
            let blocks = match HistoryMessage::of(message) {
                HistoryMessage::Assistant(blocks) => blocks,
                HistoryMessage::EnrichedAssistant(assistant) => assistant.content.as_slice(),
                HistoryMessage::AbortedAssistant(_)
                | HistoryMessage::System(_)
                | HistoryMessage::User(_)
                | HistoryMessage::CompactionSummary(_)
                | HistoryMessage::ToolResult(_)
                | HistoryMessage::ToolImageSupplement(_) => continue,
            };
            let members: Vec<EvalMember> = blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolCall(call) => Some(call),
                    _ => None,
                })
                .filter_map(|call| {
                    let (capability, summary) = replay_request(call, cwd)?;
                    Some(EvalMember {
                        id: call.id.clone(),
                        label: None,
                        pending: ApprovalRequest::new(capability, ""),
                        call_id: call.id.clone(),
                        summary,
                    })
                })
                .collect();
            if members.len() >= 2 {
                groups.push((segment_index, history, index, members));
            }
        }
    }
    let chosen: HashSet<usize> = spread_evenly(groups.len(), per_session)
        .into_iter()
        .collect();
    groups
        .into_iter()
        .enumerate()
        .filter(|(choice, _)| chosen.contains(choice))
        .map(|(_, (segment_index, history, index, members))| EvalBatch {
            id: format!("{session_id}:{segment_index}.{index}"),
            source: CaseSource::Replay,
            category: None,
            workspace: cwd.to_path_buf(),
            history: history[..=index].to_vec(),
            members,
        })
        .collect()
}

/// One line of a batch fixture file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureBatch {
    id: String,
    category: String,
    /// Why the labels are right. Read by people, not by the eval.
    #[serde(default, rename = "note")]
    _note: String,
    #[serde(default)]
    history: Vec<FixtureEntry>,
    pending: Vec<FixtureMember>,
}

/// One pending request of a fixture batch. Its fields besides `id` and
/// `label` are a case file's `pending` object, which rejects unknown fields.
#[derive(Deserialize)]
struct FixtureMember {
    id: String,
    label: Decision,
    #[serde(flatten)]
    pending: FixturePending,
}

impl FixtureBatch {
    fn into_eval_batch(self) -> EvalBatch {
        let workspace = PathBuf::from(FIXTURE_WORKSPACE);
        let mut history: Vec<Message> = self
            .history
            .into_iter()
            .map(FixtureEntry::into_message)
            .collect();
        let mut calls = Vec::new();
        let members = self
            .pending
            .into_iter()
            .map(|member| {
                let (name, arguments, capability, summary) =
                    member.pending.into_request(&workspace);
                // Prefixed so a member ID cannot collide with a history call.
                let call_id = format!("pending-{}", member.id);
                calls.push(ContentBlock::ToolCall(ToolCall {
                    id: call_id.clone(),
                    name: name.into(),
                    arguments,
                }));
                EvalMember {
                    id: member.id,
                    label: Some(member.label),
                    pending: ApprovalRequest::new(capability, ""),
                    call_id,
                    summary,
                }
            })
            .collect();
        history.push(Message::Assistant(calls));
        EvalBatch {
            id: self.id,
            source: CaseSource::Fixture,
            category: Some(self.category),
            workspace,
            history,
            members,
        }
    }
}
