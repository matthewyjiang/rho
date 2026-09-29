//! Probe questions about a session, their reference answers taken from the
//! uncompacted history, and scoring.
//!
//! References come from the whole history, not only the span a compaction
//! removed, so every configuration answers the same questions at a point and
//! one that keeps more verbatim is credited for it. The probe set is fixed per
//! [`PROBE_SET_VERSION`] so reports stay comparable. `FilesChanged` is
//! scored by exact match. The rest need a judge model, which sees only the
//! reference facts built here, never the full transcript.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rho_sdk::model::{ContentBlock, Message, ToolCall, ToolResult};
use serde::Serialize;

use crate::history_message::HistoryMessage;

/// Bump when a question, a reference, or scoring changes.
pub(super) const PROBE_SET_VERSION: u32 = 3;
/// Most recent user messages and failed tool results given to the judge.
/// Enough to cover a long session without a huge judge prompt.
const MAX_REFERENCE_ITEMS: usize = 8;
/// Characters kept from each reference item, head and tail for tool output.
const REFERENCE_ITEM_CHARS: usize = 600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Probe {
    FilesChanged,
    TestResult,
    UserRequests,
    Errors,
}

impl Probe {
    pub(super) fn question(self) -> &'static str {
        match self {
            Self::FilesChanged => {
                "List every file path the agent created, edited, or deleted in this session, one per line."
            }
            Self::TestResult => {
                "What was the most recent test, build, or lint command the agent ran (for example cargo test, cargo clippy, pytest, or make), and did it pass or fail? Quote the command."
            }
            Self::UserRequests => {
                "What has the user asked for in this session, and what constraints or preferences did they state?"
            }
            Self::Errors => {
                "What errors or failed tool calls came up in this session, and how were they handled?"
            }
        }
    }

    pub(super) fn scoring(self) -> Scoring {
        match self {
            Self::FilesChanged => Scoring::ExactMatch,
            Self::TestResult | Self::UserRequests | Self::Errors => Scoring::Judge,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scoring {
    ExactMatch,
    Judge,
}

/// What a correct answer to one probe must contain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Reference {
    /// Paths an answer must name.
    Paths(Vec<String>),
    /// Facts a judge compares the answer against.
    Facts(String),
}

impl Reference {
    pub(super) fn text(&self) -> String {
        match self {
            Self::Paths(paths) => paths.join("\n"),
            Self::Facts(facts) => facts.clone(),
        }
    }
}

/// References for every probe the history can answer. A probe with nothing
/// to ask about is left out.
pub(super) fn references(history: &[Message]) -> BTreeMap<Probe, Reference> {
    let results = history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some((result.id.as_str(), result)),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let calls = history
        .iter()
        .filter_map(Message::completed_assistant_content)
        .flatten()
        .filter_map(|block| match block {
            ContentBlock::ToolCall(call) => Some(call),
            ContentBlock::Text(_) | ContentBlock::Image(_) => None,
        })
        .collect::<Vec<_>>();
    let mut references = BTreeMap::new();

    let paths = calls
        .iter()
        .flat_map(|call| changed_paths(call))
        .collect::<BTreeSet<_>>();
    if !paths.is_empty() {
        references.insert(
            Probe::FilesChanged,
            Reference::Paths(paths.into_iter().collect()),
        );
    }

    if let Some((call, result)) = calls.iter().rev().find_map(|call| {
        let command = shell_command(call)?;
        is_check_command(command).then_some((command, results.get(call.id.as_str())?))
    }) {
        references.insert(
            Probe::TestResult,
            Reference::Facts(format!(
                "command: {}\nresult: {}",
                excerpt(call),
                status(result)
            )),
        );
    }

    let requests = history
        .iter()
        .filter_map(|message| match HistoryMessage::of(message) {
            HistoryMessage::User(blocks) => Some(text_of(blocks)),
            _ => None,
        })
        .filter(|text| !text.trim().is_empty() && !is_host_context(text))
        .collect::<Vec<_>>();
    if !requests.is_empty() {
        references.insert(
            Probe::UserRequests,
            Reference::Facts(numbered(requests.iter().map(|text| excerpt(text)))),
        );
    }

    let errors = calls
        .iter()
        .filter(|call| is_consequential(call))
        .filter_map(|call| {
            let result = results.get(call.id.as_str())?;
            (!result.ok).then(|| {
                format!(
                    "{} {}: {}",
                    call.name,
                    excerpt(&call.arguments.to_string()),
                    excerpt(&result.content)
                )
            })
        })
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        references.insert(
            Probe::Errors,
            Reference::Facts(numbered(errors.into_iter())),
        );
    }
    references
}

/// Fraction of `paths` the answer names. A path counts when the answer holds
/// its last two components, so absolute and relative spellings both match.
pub(super) fn path_recall(paths: &[String], answer: &str) -> f64 {
    if paths.is_empty() {
        return 0.0;
    }
    let found = paths
        .iter()
        .filter(|path| answer.contains(&path_suffix(path)))
        .count();
    found as f64 / paths.len() as f64
}

fn path_suffix(path: &str) -> String {
    let parts = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    parts[parts.len().saturating_sub(2)..].join("/")
}

/// Paths a file-changing tool call names. Covers every edit format.
fn changed_paths(call: &ToolCall) -> Vec<String> {
    let string = |key: &str| call.arguments.get(key).and_then(|value| value.as_str());
    match call.name.as_str() {
        "write" | "str_replace" => string("path").map(str::to_owned).into_iter().collect(),
        // Hashline sections start with `[path#TAG]`.
        "edit" => string("input")
            .into_iter()
            .flat_map(str::lines)
            .filter_map(|line| {
                let header = line.trim().strip_prefix('[')?.strip_suffix(']')?;
                Some(header.rsplit_once('#')?.0.to_owned())
            })
            .collect(),
        "apply_patch" => string("input")
            .into_iter()
            .flat_map(str::lines)
            .filter_map(|line| {
                [
                    "*** Add File: ",
                    "*** Update File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .map(|path| path.trim().to_owned())
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// User-role text the host injected rather than a person typed: tool and
/// agent notifications and runtime context updates. Each starts with a
/// bracketed header such as `[process notification]`.
fn is_host_context(text: &str) -> bool {
    const HEADERS: [&str; 5] = [
        "[process notification]",
        "[agent notification]",
        "[runtime notifications ",
        "[computer use context]",
        "[conversation model switched ",
    ];
    let text = text.trim_start();
    HEADERS.iter().any(|header| text.starts_with(header))
}

/// Failed calls an agent must remember: a failing check, or a file change
/// that did not apply. A failed `grep` or `ls` is routine exploration.
fn is_consequential(call: &ToolCall) -> bool {
    !changed_paths(call).is_empty() || shell_command(call).is_some_and(is_check_command)
}

fn shell_command(call: &ToolCall) -> Option<&str> {
    matches!(call.name.as_str(), "bash" | "powershell")
        .then(|| call.arguments.get("command")?.as_str())
        .flatten()
}

/// Commands whose pass or fail status an agent must remember: a segment of
/// the first command line (split on `&&`, `||`, `;`, and `|`) that starts
/// with a known test, build, or lint invocation. Matching whole invocations
/// rather than words keeps `git diff --check`, `gh pr create`, and commit
/// messages that mention tests from counting.
fn is_check_command(command: &str) -> bool {
    const INVOCATIONS: [&[&str]; 16] = [
        &["cargo", "test"],
        &["cargo", "nextest"],
        &["cargo", "clippy"],
        &["cargo", "check"],
        &["cargo", "build"],
        &["pytest"],
        &["python3", "-m", "pytest"],
        &["python3", "-m", "unittest"],
        &["python3", "scripts/validate.py"],
        &["npm", "test"],
        &["npm", "run"],
        &["pnpm", "test"],
        &["go", "test"],
        &["make"],
        &["tsc"],
        &["vitest"],
    ];
    // Only the first line: later lines of a multi-line command are usually
    // heredoc or commit-message text, not commands.
    command
        .lines()
        .next()
        .unwrap_or_default()
        .split([';', '|', '&'])
        .map(|segment| {
            segment
                .split_whitespace()
                // Environment assignments and `timeout N` prefixes.
                .skip_while(|word| word.contains('=') || *word == "timeout")
                .skip_while(|word| word.chars().all(|c| c.is_ascii_digit() || c == 's'))
                .collect::<Vec<_>>()
        })
        .any(|words| {
            INVOCATIONS
                .iter()
                .any(|invocation| words.starts_with(invocation))
        })
}

fn status(result: &ToolResult) -> &'static str {
    if result.ok {
        "passed"
    } else {
        "failed"
    }
}

fn text_of(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::Image(_) | ContentBlock::ToolCall(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keeps the most recent items, numbered oldest first.
fn numbered(items: impl DoubleEndedIterator<Item = String>) -> String {
    let mut recent = items.rev().take(MAX_REFERENCE_ITEMS).collect::<Vec<_>>();
    recent.reverse();
    recent
        .iter()
        .enumerate()
        .map(|(index, item)| format!("{}. {item}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Head and tail of `text`, which carry the command echo and the verdict.
fn excerpt(text: &str) -> String {
    let chars = text.chars().count();
    if chars <= REFERENCE_ITEM_CHARS {
        return text.to_owned();
    }
    let half = REFERENCE_ITEM_CHARS / 2;
    let head = text.chars().take(half).collect::<String>();
    let tail = text.chars().skip(chars - half).collect::<String>();
    format!("{head}\n[...]\n{tail}")
}

#[cfg(test)]
#[path = "probes_tests.rs"]
mod tests;
