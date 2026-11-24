use rho_tools::tool::truncate;

use {
    super::{
        agent::{SubagentNotification, SubagentSnapshot},
        notification_format::{bounded_notification, detail_line, Excerpt, RESULT_EXCERPT_BYTES},
    },
    crate::subagent::{self, RunState},
};

const SUMMARY_DETAIL_BYTES: usize = 256;

const NOTIFICATION_HEADER: &str = "[agent notification]\n\nRun status:\n";
const NOTIFICATION_FOOTER: &str = "\n\nAny omitted or truncated result details remain available through `agents list` or `agents status`.\n\nThis is an automated notification, not a user message. Fold the results into your ongoing work; use the agents tool for details.";

#[derive(Clone, Copy)]
pub(super) enum SnapshotFormat {
    Completion,
    Status,
}

pub(super) fn format_background_start(id: &str, agent_id: &str) -> String {
    format!("agent {id} ({agent_id}) started in background")
}

/// Script view of a run. Running output stays private; final answers share
/// the model text's excerpt budget.
#[derive(serde::Serialize, schemars::JsonSchema)]
pub(super) struct AgentRunView {
    id: String,
    agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    state: RunState,
    done: bool,
    result: Option<String>,
    error: Option<String>,
}

impl From<&SubagentSnapshot> for AgentRunView {
    fn from(snapshot: &SubagentSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            agent_id: snapshot.agent_id.clone(),
            provider: snapshot.status.provider.clone(),
            model: snapshot.status.model.clone(),
            state: snapshot.status.state,
            done: snapshot.done,
            result: snapshot
                .status
                .result
                .as_deref()
                .filter(|result| snapshot.done && !result.is_empty())
                .map(|result| truncate(result.to_string(), RESULT_EXCERPT_BYTES)),
            error: snapshot.status.error.clone(),
        }
    }
}

impl AgentRunView {
    /// A launch receipt is not a completion observation, even if the child has
    /// already finished. Its result still arrives through normal delivery.
    pub(super) fn started_with_identity(snapshot: &SubagentSnapshot) -> Self {
        Self {
            provider: snapshot.status.provider.clone(),
            model: snapshot.status.model.clone(),
            ..Self::started(snapshot.id.clone(), snapshot.agent_id.clone())
        }
    }

    /// A receipt for run `id` of agent `agent_id` whose model only the child
    /// knows, such as a host thread's.
    pub(super) fn started(id: String, agent_id: String) -> Self {
        Self {
            id,
            agent_id,
            provider: None,
            model: None,
            state: RunState::Starting,
            done: false,
            result: None,
            error: None,
        }
    }
}

pub(super) fn format_snapshot(snapshot: &SubagentSnapshot, format: SnapshotFormat) -> String {
    let mut lines = vec![format!(
        "agent {} ({}): {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str()
    )];
    let metrics = format!(
        "turns: {} · tokens: {} in / {} out",
        snapshot.status.turns,
        format_token_count(snapshot.status.input_tokens),
        format_token_count(snapshot.status.output_tokens)
    );
    match format {
        SnapshotFormat::Completion => lines.push(metrics),
        SnapshotFormat::Status => {
            lines.push(format!(
                "elapsed: {} · {metrics}",
                subagent::format_elapsed_secs(snapshot.elapsed.as_secs())
            ));
            // Progress metadata only: a running run's streamed text stays
            // private so the parent cannot act on results before delivery.
            if !snapshot.done {
                if let Some(activity) = &snapshot.status.last_activity {
                    lines.push(format!("activity: {activity}"));
                }
            }
        }
    }
    lines.extend(run_model_line(&snapshot.status));
    push_cli_metadata(&mut lines, snapshot);
    if matches!(format, SnapshotFormat::Completion) {
        if let Some(error) = &snapshot.status.error {
            lines.push(format!("error: {error}"));
        }
        if matches!(snapshot.status.state, RunState::Error | RunState::Stopped) {
            lines.push("this delegated task did not complete; treat its work as unverified".into());
        }
    }
    if matches!(format, SnapshotFormat::Completion) || !snapshot.done {
        if let Some(error) = &snapshot.status.attachment_error {
            lines.push(format!("attachment error: {error}"));
        }
    }
    if snapshot.done && matches!(format, SnapshotFormat::Completion) {
        if let Some(result) = snapshot
            .status
            .result
            .as_deref()
            .filter(|result| !result.is_empty())
        {
            lines.push(String::new());
            lines.push(truncate(result.to_string(), RESULT_EXCERPT_BYTES));
        }
    }
    if !snapshot.prior_notices.is_empty() {
        lines.push("Earlier child notices, in send order. This terminal result supersedes their progress and planning; retain any substantive findings:".into());
        lines.push(truncate(
            snapshot.prior_notices.join("\n"),
            RESULT_EXCERPT_BYTES,
        ));
    }
    lines.join("\n")
}

/// Formats one bounded model notification. Run summaries take priority over
/// result excerpts so a large result cannot hide the state of later runs.
pub(super) fn format_notification(snapshots: &[&SubagentSnapshot]) -> String {
    let excerpts = snapshots
        .iter()
        .filter_map(|snapshot| {
            let result = snapshot.status.result.as_deref()?;
            (!result.is_empty()).then(|| Excerpt {
                label: format!("\n\nagent {}:\n", snapshot.id),
                text: result,
                close: String::new(),
            })
        })
        .collect();
    bounded_notification(
        NOTIFICATION_HEADER,
        NOTIFICATION_FOOTER,
        snapshots
            .iter()
            .map(|snapshot| completion_summary(snapshot).join("\n")),
        |remaining| format!("... {remaining} run status section(s) omitted; use `agents status`"),
        "\n\nResult excerpts:",
        excerpts,
    )
}

/// Formats a drained batch of terminal runs as one bounded notification. The
/// formatter puts every run's status before the result excerpts.
pub(crate) fn notification_prompt(notifications: &[SubagentNotification]) -> String {
    let snapshots = notifications
        .iter()
        .map(|notification| &notification.snapshot)
        .collect::<Vec<_>>();
    format_notification(&snapshots)
}

fn completion_summary(snapshot: &SubagentSnapshot) -> Vec<String> {
    let mut lines = vec![format!(
        "agent {} ({}): {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str()
    )];
    lines.push(format!(
        "turns: {} · tokens: {} in / {} out",
        snapshot.status.turns,
        format_token_count(snapshot.status.input_tokens),
        format_token_count(snapshot.status.output_tokens)
    ));
    lines.extend(run_model_line(&snapshot.status));
    push_cli_metadata(&mut lines, snapshot);
    if let Some(error) = &snapshot.status.error {
        lines.push(format!(
            "error: {}",
            truncate(error.clone(), SUMMARY_DETAIL_BYTES)
        ));
    }
    if matches!(snapshot.status.state, RunState::Error | RunState::Stopped) {
        lines.push("this delegated task did not complete; treat its work as unverified".into());
    }
    if let Some(error) = &snapshot.status.attachment_error {
        lines.push(format!(
            "attachment error: {}",
            truncate(error.clone(), SUMMARY_DETAIL_BYTES)
        ));
    }
    lines
}

/// Which model a run used, from what the run recorded.
///
/// The agent list stays model-free on purpose: which model an agent runs on can
/// change after that list is written. A run reports its own model instead, where
/// the answer is settled and cannot go stale.
fn run_model_line(status: &crate::subagent::RunStatus) -> Option<String> {
    Some(format!(
        "model: {}",
        crate::model_identity::PromptModel::from_run_status(status)?.describe()
    ))
}

fn push_cli_metadata(lines: &mut Vec<String>, snapshot: &SubagentSnapshot) {
    use crate::{
        agent::AgentRuntime, antigravity_runtime::ANTIGRAVITY_LABEL,
        claude_runtime::session::CLAUDE_LABEL, cursor_runtime::models::CURSOR_LABEL,
        session::SessionPersistence,
    };

    let label = match snapshot.status.runtime {
        Some(AgentRuntime::Cursor) => CURSOR_LABEL,
        Some(AgentRuntime::Antigravity) => ANTIGRAVITY_LABEL,
        Some(AgentRuntime::ClaudeCli) | Some(AgentRuntime::Rho) | None => CLAUDE_LABEL,
    };
    // A run that saved nothing (`rho run --no-save`) discarded its session.
    let resume_command = match snapshot.persistence {
        SessionPersistence::Saved => label.resume_command,
        SessionPersistence::Ephemeral => None,
    };
    if let Some(session_id) = &snapshot.status.runtime_session_id {
        let session_label = label.session_label;
        lines.push(match resume_command {
            Some(command) => format!(
                "{session_label}: {session_id} (resume with `{command} --resume {session_id}`)"
            ),
            None => format!("{session_label}: {session_id}"),
        });
    }
    if let Some(cost) = snapshot.status.total_cost_usd {
        lines.push(format!("{}: ${cost:.4}", label.cost_label));
    }
}

pub(super) fn format_list_entry(snapshot: &SubagentSnapshot) -> String {
    // Activity labels only: streamed text stays out of model-facing listings
    // so results are consumed through delivery, not previews.
    let detail = snapshot
        .status
        .last_activity
        .as_deref()
        .unwrap_or(if snapshot.done { "finished" } else { "working" });
    format!(
        "{}  {}  {}  {}  {}",
        snapshot.id,
        snapshot.agent_id,
        snapshot.status.state.as_str(),
        subagent::format_elapsed_secs(snapshot.elapsed.as_secs()),
        detail_line(detail)
    )
}

fn format_token_count(tokens: Option<u64>) -> String {
    tokens.map_or_else(|| "?".into(), |tokens| tokens.to_string())
}

#[cfg(test)]
#[path = "agent_output_tests.rs"]
mod tests;
