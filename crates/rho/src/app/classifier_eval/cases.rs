//! Eval cases: labeled fixtures and calls replayed from saved sessions.
//!
//! Both sources build the pending request the way the production `bash`,
//! `write`, and `read_file` tools do, with the policy's empty approval reason,
//! and end the history with the unanswered call being classified.
//!
//! One approximation remains: production resolves symlinks before scoping a
//! path, and the eval scopes paths lexically. A replayed path that went
//! through a symlink can land in a different scope than it did live.

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use rho_providers::model::{ContentBlock, Message, ToolCall, ToolResult};
use rho_sdk::{
    ApprovalRequest, CapabilityRequest, CapabilitySource, PathScope, ProcessEnvironment,
    ProcessExecution, ProcessInvocation, ProcessOutputLimits,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    history_message::HistoryMessage,
    session::replay_points::{self, HistorySegment},
};

/// Workspace root every fixture case runs in.
const FIXTURE_WORKSPACE: &str = "/workspace";
/// Call ID of the pending call appended to fixture histories.
const FIXTURE_PENDING_CALL_ID: &str = "pending";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Decision {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum CaseSource {
    Fixture,
    Replay,
}

/// One pending request with the history the classifier sees for it.
#[derive(Clone, Debug)]
pub(super) struct EvalCase {
    pub id: String,
    pub source: CaseSource,
    /// Fixture category, such as `scope_creep`. Replayed cases have none.
    pub category: Option<String>,
    /// Expected decision. Replayed cases are unlabeled.
    pub label: Option<Decision>,
    pub workspace: PathBuf,
    pub history: Vec<Message>,
    pub pending: ApprovalRequest,
    /// ID of the call being classified, which the SDK would attach to
    /// `pending` in production.
    pub pending_call_id: String,
    /// The pending command or path, for reading reports.
    pub summary: String,
}

/// Loads a JSON Lines fixture file. Blank lines are skipped; IDs must be unique.
pub(super) fn load_fixture_file(path: &Path) -> anyhow::Result<Vec<EvalCase>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()))?;
    parse_fixture_cases(&text).with_context(|| format!("invalid case file {}", path.display()))
}

pub(super) fn parse_fixture_cases(text: &str) -> anyhow::Result<Vec<EvalCase>> {
    let mut ids = HashSet::new();
    let mut cases = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_number = index + 1;
        let case: FixtureCase =
            serde_json::from_str(line).with_context(|| format!("line {line_number}"))?;
        anyhow::ensure!(
            ids.insert(case.id.clone()),
            "line {line_number}: duplicate case id {}",
            case.id
        );
        cases.push(case.into_eval_case());
    }
    Ok(cases)
}

/// Replays up to `per_session` eligible calls from a saved session, spread
/// evenly through its whole active path.
pub(super) fn replay_session(path: &Path, per_session: usize) -> anyhow::Result<Vec<EvalCase>> {
    let (session_id, segments) = replay_points::segments(path)?;
    let cwd = replay_points::session_cwd(path)?;
    Ok(replay_cases(&session_id, &cwd, &segments, per_session))
}

/// Each case's history ends with the assistant message holding its call, so
/// the call is unanswered, as it is when approval is requested. A call is
/// replayed from the compaction segment it was made in, after any summary
/// that preceded it. Aborted calls never asked for approval and are skipped.
pub(super) fn replay_cases(
    session_id: &str,
    cwd: &Path,
    segments: &[HistorySegment],
    per_session: usize,
) -> Vec<EvalCase> {
    let mut eligible = Vec::new();
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
            for block in blocks {
                let ContentBlock::ToolCall(call) = block else {
                    continue;
                };
                if let Some((capability, summary)) = replay_request(call, cwd) {
                    eligible.push((segment_index, history, index, &call.id, capability, summary));
                }
            }
        }
    }
    spread_evenly(eligible.len(), per_session)
        .into_iter()
        .map(|choice| {
            let (segment_index, history, index, call_id, capability, summary) = &eligible[choice];
            EvalCase {
                // Call IDs can repeat across responses; the segment and message
                // index keep the case ID unique.
                id: format!("{session_id}:{segment_index}.{index}:{call_id}"),
                source: CaseSource::Replay,
                category: None,
                label: None,
                workspace: cwd.to_path_buf(),
                history: history[..=*index].to_vec(),
                pending: ApprovalRequest::new(capability.clone(), ""),
                pending_call_id: (*call_id).clone(),
                summary: summary.clone(),
            }
        })
        .collect()
}

/// The request a replayed call made, for calls that can reach the classifier
/// in Auto mode: every `bash` command and every `write`. Edits to existing
/// tracked files usually pass the Allow edits gate, so `str_replace` and
/// friends are not replayed. Calls whose arguments the tool rejects before
/// asking for approval are skipped.
fn replay_request(call: &ToolCall, cwd: &Path) -> Option<(CapabilityRequest, String)> {
    let arguments = call.arguments.clone();
    match call.name.as_str() {
        "bash" => {
            let args: ReplayedBash = serde_json::from_value(arguments).ok()?;
            (args.timeout_seconds != Some(0))
                .then(|| (process_request(cwd, &args.command), args.command))
        }
        "write" => {
            let args: ReplayedWrite = serde_json::from_value(arguments).ok()?;
            Some(path_request(PathAccess::Write, cwd, &args.path))
        }
        _ => None,
    }
}

/// Mirrors the `bash` tool's argument checks: a string command, and a
/// timeout that is absent or positive.
#[derive(Deserialize)]
struct ReplayedBash {
    command: String,
    timeout_seconds: Option<u64>,
}

/// Mirrors the `write` tool's arguments, which require both fields.
#[derive(Deserialize)]
struct ReplayedWrite {
    path: String,
    #[serde(rename = "content")]
    _content: String,
}

/// Up to `count` indexes into `len` items, spread evenly from first to last.
fn spread_evenly(len: usize, count: usize) -> Vec<usize> {
    if len <= count {
        return (0..len).collect();
    }
    let last = len - 1;
    let mut chosen = (0..count)
        .map(|step| match count {
            1 => last,
            _ => step * last / (count - 1),
        })
        .collect::<Vec<_>>();
    chosen.dedup();
    chosen
}

fn process_request(cwd: &Path, command: &str) -> CapabilityRequest {
    let execution = ProcessExecution::new(
        cwd,
        ProcessInvocation::shell_from_path("bash", vec!["-lc".into()], command),
        ProcessEnvironment::InheritAll,
        // The transcript shows only the working directory and command.
        ProcessOutputLimits::new(/*max_output_bytes*/ 0, /*timeout*/ None),
    );
    CapabilityRequest::process(execution, CapabilitySource::built_in_tool("bash"))
}

#[derive(Clone, Copy)]
enum PathAccess {
    Read,
    Write,
}

/// Resolves `raw` against `workspace` and scopes it the way workspace path
/// resolution would: inside the workspace is the primary workspace, anything
/// else is unrestricted. Returns the request and the resolved path.
fn path_request(access: PathAccess, workspace: &Path, raw: &str) -> (CapabilityRequest, String) {
    let path = normalize_lexically(&workspace.join(raw));
    let scope = if path.starts_with(workspace) {
        PathScope::PrimaryWorkspace
    } else {
        PathScope::UnrestrictedFilesystem
    };
    let summary = path.to_string_lossy().into_owned();
    let request = match access {
        PathAccess::Read => {
            CapabilityRequest::read_path(path, scope, CapabilitySource::built_in_tool("read_file"))
        }
        PathAccess::Write => {
            CapabilityRequest::write_path(path, scope, CapabilitySource::built_in_tool("write"))
        }
    };
    (request, summary)
}

/// Folds `.` and `..` without touching the filesystem, so `../x` cannot pass
/// for a workspace path.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component)
            }
        }
    }
    normalized
}

/// One line of a fixture file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureCase {
    id: String,
    label: Decision,
    category: String,
    /// Why the label is right. Read by people, not by the eval.
    #[serde(default, rename = "note")]
    _note: String,
    #[serde(default)]
    history: Vec<FixtureEntry>,
    pending: FixturePending,
}

impl FixtureCase {
    fn into_eval_case(self) -> EvalCase {
        let workspace = PathBuf::from(FIXTURE_WORKSPACE);
        let mut history: Vec<Message> = self
            .history
            .into_iter()
            .map(FixtureEntry::into_message)
            .collect();
        let (name, arguments, capability, summary) = self.pending.into_request(&workspace);
        history.push(Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
            id: FIXTURE_PENDING_CALL_ID.into(),
            name: name.into(),
            arguments,
        })]));
        EvalCase {
            id: self.id,
            source: CaseSource::Fixture,
            category: Some(self.category),
            label: Some(self.label),
            workspace,
            history,
            pending: ApprovalRequest::new(capability, ""),
            pending_call_id: FIXTURE_PENDING_CALL_ID.into(),
            summary,
        }
    }
}

/// History before the pending call: user text, an assistant tool call, or a
/// tool result.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum FixtureEntry {
    User(String),
    Call {
        id: String,
        name: String,
        arguments: Value,
    },
    Result {
        id: String,
        content: String,
        #[serde(default = "result_ok_default")]
        ok: bool,
    },
}

fn result_ok_default() -> bool {
    true
}

impl FixtureEntry {
    fn into_message(self) -> Message {
        match self {
            Self::User(text) => Message::User(vec![ContentBlock::Text(text)]),
            Self::Call {
                id,
                name,
                arguments,
            } => Message::Assistant(vec![ContentBlock::ToolCall(ToolCall {
                id,
                name,
                arguments,
            })]),
            Self::Result { id, content, ok } => Message::ToolResult(ToolResult { id, ok, content }),
        }
    }
}

/// The call under review. Paths resolve against the fixture workspace.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FixturePending {
    Process {
        command: String,
    },
    Write {
        path: String,
        #[serde(default)]
        content: String,
    },
    Read {
        path: String,
    },
}

impl FixturePending {
    /// Tool name, call arguments, capability request, and report summary.
    fn into_request(self, workspace: &Path) -> (&'static str, Value, CapabilityRequest, String) {
        match self {
            Self::Process { command } => (
                "bash",
                json!({ "command": command }),
                process_request(workspace, &command),
                command,
            ),
            Self::Write { path, content } => {
                let (request, summary) = path_request(PathAccess::Write, workspace, &path);
                (
                    "write",
                    json!({ "path": path, "content": content }),
                    request,
                    summary,
                )
            }
            Self::Read { path } => {
                let (request, summary) = path_request(PathAccess::Read, workspace, &path);
                ("read_file", json!({ "path": path }), request, summary)
            }
        }
    }
}
