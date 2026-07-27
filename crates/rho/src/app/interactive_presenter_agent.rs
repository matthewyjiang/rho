use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolHeader, ToolStatus};

use super::{format::draft_card, ToolView};

const TASK_PREVIEW_BYTES: usize = 160;
/// Live agent prompts are long; show a trailing window so argument streaming keeps
/// moving instead of freezing on a short prefix summary.
const STREAMING_PROMPT_CHARS: usize = 400;
const STREAMING_PROMPT_LINES: usize = 8;

pub(super) fn agent_start_card(arguments: &serde_json::Value) -> ToolCard {
    agent_card(
        arguments,
        ToolStatus::Running,
        agent_identity(arguments).unwrap_or("agent"),
        starting_detail(bool_value(arguments, "background")),
    )
}

/// Streaming preview for an in-progress `agent` tool call.
///
/// Uses the shared incomplete-JSON path so agent previews share one parser and
/// the same large-buffer stride as other tools.
pub(super) fn agent_streaming_preview_card(arguments: &serde_json::Value) -> ToolCard {
    let agent_id = string_value(arguments, "agent_id")
        .filter(|id| !id.is_empty())
        .unwrap_or("agent");
    let background = bool_value(arguments, "background");
    let mut card = bare_agent_card(ToolStatus::Running, agent_id, starting_detail(background));
    if let Some(prompt) = string_value(arguments, "prompt").filter(|prompt| !prompt.is_empty()) {
        for line in live_tail_prompt_lines(prompt) {
            card.push_fact(ToolFact::Text { text: line });
        }
    }
    card
}

pub(super) fn agent_interrupted_card(arguments: &serde_json::Value) -> ToolCard {
    agent_card(
        arguments,
        ToolStatus::Interrupted,
        agent_identity(arguments).unwrap_or("agent"),
        "interrupted",
    )
}

pub(super) fn agents_interrupted_card(arguments: &serde_json::Value) -> ToolCard {
    let action = string_value(arguments, "action").unwrap_or("request");
    bare_agent_card(
        ToolStatus::Interrupted,
        string_value(arguments, "id").unwrap_or("delegated agents"),
        format!("{action} interrupted"),
    )
}

pub(super) fn agent_progress_card(view: &ToolView, content: &str) -> ToolCard {
    let agent_id = agent_identity(&view.arguments).unwrap_or("agent");
    let mut card = agent_card(&view.arguments, ToolStatus::Running, agent_id, "running");
    if let Some(run_id) = run_id_from_agent_line(content.lines().next().unwrap_or_default()) {
        card.body = ToolBody::Lines(vec![format!("{run_id} · rho attach {run_id}")]);
    }
    card
}

pub(super) fn agent_finished_card(view: &ToolView, content: &str, ok: bool) -> ToolCard {
    if let (true, Some(receipt)) = (ok, parse_background_receipt(content)) {
        let mut card = agent_card(
            &view.arguments,
            ToolStatus::Running,
            receipt.agent_id,
            "running in background",
        );
        card.body = ToolBody::Lines(vec![format!(
            "{} · rho attach {}",
            receipt.run_id, receipt.run_id
        )]);
        return card;
    }
    if let Some(snapshot) = parse_snapshot(content) {
        return snapshot_card(
            view,
            snapshot,
            SnapshotDisplay::Completion,
            ToolStatus::from_finished(ok),
        );
    }

    let status = ToolStatus::from_finished(ok);
    let mut card = agent_card(
        &view.arguments,
        status,
        agent_identity(&view.arguments).unwrap_or("agent"),
        if ok { "completed" } else { "failed" },
    );
    set_content_body(&mut card, content);
    card
}

pub(super) fn agents_start_card(arguments: &serde_json::Value) -> ToolCard {
    let (identity, detail) = match string_value(arguments, "action") {
        Some("list") => ("delegated agents", "loading"),
        Some("status") => (
            string_value(arguments, "id").unwrap_or("delegated agent"),
            "checking status",
        ),
        Some("stop") => (
            string_value(arguments, "id").unwrap_or("delegated agent"),
            "stopping",
        ),
        Some(action) => ("agents", action),
        None => ("agents", "ready"),
    };
    bare_agent_card(ToolStatus::Running, identity, detail)
}

pub(super) fn agents_finished_card(view: &ToolView, content: &str, ok: bool) -> ToolCard {
    if !ok {
        let action = string_argument(view, "action").unwrap_or("request");
        let mut card = bare_agent_card(ToolStatus::Error, "agents", format!("{action} failed"));
        set_content_body(&mut card, content);
        return card;
    }

    match string_argument(view, "action") {
        Some("list") => {
            let mut lines = agent_list_lines(content).into_iter();
            let mut card = bare_agent_card(ToolStatus::Ok, "delegated agents", "");
            lines.next();
            for text in lines {
                card.push_fact(ToolFact::Text {
                    text: text.trim_start().to_string(),
                });
            }
            card
        }
        Some(action @ ("status" | "stop")) => parse_snapshot(content)
            .map(|snapshot| {
                let display = if action == "status" || snapshot.has_status_metrics() {
                    SnapshotDisplay::Status
                } else {
                    SnapshotDisplay::Completion
                };
                snapshot_card(view, snapshot, display, ToolStatus::Ok)
            })
            .unwrap_or_else(|| agents_result_fallback_card(view, content)),
        _ => {
            let mut card = bare_agent_card(ToolStatus::Ok, "agents", "result");
            set_content_body(&mut card, content);
            card
        }
    }
}

fn agent_card(
    arguments: &serde_json::Value,
    status: ToolStatus,
    identity: impl Into<String>,
    detail: impl Into<String>,
) -> ToolCard {
    let mut card = bare_agent_card(status, identity, detail);
    if let Some(task) = task_preview(arguments) {
        push_agent_fact(&mut card, task);
    }
    card
}

fn bare_agent_card(
    status: ToolStatus,
    identity: impl Into<String>,
    detail: impl Into<String>,
) -> ToolCard {
    draft_card(
        status,
        rho_tools::tool_card::ToolFamily::Agent,
        ToolHeader::status_first(identity, detail),
    )
}

fn push_agent_fact(card: &mut ToolCard, text: String) {
    if card.status == ToolStatus::Error
        || text.starts_with("error:")
        || text.starts_with("attachment error:")
    {
        card.push_fact(ToolFact::Error { text });
    } else {
        card.push_fact(ToolFact::Text { text });
    }
}

fn task_preview(arguments: &serde_json::Value) -> Option<String> {
    let task = string_value(arguments, "prompt")?
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!task.is_empty()).then(|| truncate_preview(&task))
}

fn starting_detail(background: bool) -> &'static str {
    if background {
        "starting in background"
    } else {
        "starting"
    }
}

fn agents_result_fallback_card(view: &ToolView, content: &str) -> ToolCard {
    let action = string_argument(view, "action").unwrap_or("request");
    let mut card = bare_agent_card(
        ToolStatus::Ok,
        string_argument(view, "id").unwrap_or("agents"),
        format!("{action} result"),
    );
    set_content_body(&mut card, content);
    card
}

fn set_content_body(card: &mut ToolCard, content: &str) {
    if !content.trim().is_empty() {
        card.body = ToolBody::Lines(content.lines().map(str::to_string).collect());
    }
}

fn live_tail_prompt_lines(task: &str) -> Vec<String> {
    // Walk backward once so long prompts do not pay a full char count + rescan.
    let mut kept_chars = 0usize;
    let mut start = 0usize;
    let mut dropped_chars = false;
    for (index, _) in task.char_indices().rev() {
        kept_chars += 1;
        start = index;
        if kept_chars == STREAMING_PROMPT_CHARS {
            dropped_chars = index > 0;
            break;
        }
    }
    let body = &task[start..];

    let raw_lines = body.lines().collect::<Vec<_>>();
    let dropped_lines = raw_lines.len() > STREAMING_PROMPT_LINES;
    let kept = if dropped_lines {
        &raw_lines[raw_lines.len() - STREAMING_PROMPT_LINES..]
    } else {
        raw_lines.as_slice()
    };
    if kept.is_empty() {
        return Vec::new();
    }

    let mark_omission = dropped_chars || dropped_lines;
    kept.iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 && mark_omission {
                format!("…{}", line.trim_start())
            } else {
                (*line).to_string()
            }
        })
        .collect()
}

fn truncate_preview(text: &str) -> String {
    if text.len() <= TASK_PREVIEW_BYTES {
        return text.to_string();
    }
    let mut boundary = TASK_PREVIEW_BYTES;
    while boundary > 0 && !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let prefix = &text[..boundary];
    let boundary = prefix
        .char_indices()
        .rev()
        .find_map(|(index, character)| character.is_whitespace().then_some(index))
        .unwrap_or(boundary);
    format!("{}…", text[..boundary].trim_end())
}

fn agent_list_lines(content: &str) -> Vec<String> {
    let mut lines = vec!["delegated agents".into()];
    if matches!(content.trim(), "no delegated agents" | "no subagents") {
        lines.push("  no runs".into());
        return lines;
    }
    lines.extend(content.lines().map(|line| {
        let fields = line.splitn(5, "  ").collect::<Vec<_>>();
        if fields.len() != 5 {
            return format!("  {line}");
        }
        format!(
            "{} {}  {}  {}  {}  {}",
            state_glyph(fields[2]),
            fields[0],
            fields[1],
            display_state(fields[2]),
            fields[3],
            fields[4]
        )
    }));
    lines
}

struct BackgroundReceipt<'a> {
    run_id: &'a str,
    agent_id: &'a str,
}

fn parse_background_receipt(content: &str) -> Option<BackgroundReceipt<'_>> {
    let first = content.lines().next()?;
    let rest = strip_run_prefix(first)?;
    let (run_id, rest) = rest.split_once(" (")?;
    let (agent_id, suffix) = rest.split_once(") ")?;
    (suffix == "started in background").then_some(BackgroundReceipt { run_id, agent_id })
}

struct Snapshot<'a> {
    run_id: &'a str,
    agent_id: &'a str,
    state: &'a str,
    remaining: Vec<&'a str>,
}

impl Snapshot<'_> {
    fn has_status_metrics(&self) -> bool {
        self.remaining
            .iter()
            .any(|line| line.starts_with("elapsed: ") || line.starts_with("attach: "))
    }
}

#[derive(Clone, Copy)]
enum SnapshotDisplay {
    Completion,
    Status,
}

fn parse_snapshot(content: &str) -> Option<Snapshot<'_>> {
    let mut lines = content.split('\n');
    let first = lines.next()?;
    let rest = strip_run_prefix(first)?;
    let (run_id, rest) = rest.split_once(" (")?;
    let (agent_id, state) = rest.split_once("): ")?;
    Some(Snapshot {
        run_id,
        agent_id,
        state,
        remaining: lines.collect(),
    })
}

fn snapshot_card(
    view: &ToolView,
    snapshot: Snapshot<'_>,
    display: SnapshotDisplay,
    fallback_status: ToolStatus,
) -> ToolCard {
    let metrics_index = snapshot
        .remaining
        .iter()
        .position(|line| line.starts_with("turns: ") || line.starts_with("elapsed: "));
    let metrics = metrics_index.map(|index| snapshot.remaining[index]);
    let mut detail = vec![display_state(snapshot.state).to_string()];
    detail.extend(metrics.and_then(elapsed_from_metrics).map(str::to_string));
    detail.extend(metrics.and_then(turns_from_metrics));
    let mut card = agent_card(
        &view.arguments,
        snapshot_status(snapshot.state, fallback_status),
        snapshot.agent_id,
        detail.join(" · "),
    );

    let tokens = metrics.and_then(tokens_from_metrics);
    let attach = snapshot
        .remaining
        .iter()
        .find_map(|line| line.strip_prefix("attach: "));
    let (summary_lines, result_lines) =
        snapshot_sections(&snapshot.remaining, metrics_index, display);
    for text in summary_lines {
        if !text.is_empty() {
            push_agent_fact(&mut card, text.trim_start().to_string());
        }
    }

    let mut body = Vec::new();
    if tokens.is_some() || attach.is_some() || !snapshot.run_id.is_empty() {
        body.push(match (tokens, attach) {
            (Some(tokens), _) => format!("{} · {tokens}", snapshot.run_id),
            (None, Some(attach)) => format!("{} · {attach}", snapshot.run_id),
            (None, None) => snapshot.run_id.to_string(),
        });
        if tokens.is_some() {
            if let Some(attach) = attach {
                body.push(attach.to_string());
            }
        }
    }
    body.extend(result_lines);
    if !body.is_empty() {
        card.body = ToolBody::Lines(body);
    }
    card
}

fn snapshot_status(state: &str, fallback: ToolStatus) -> ToolStatus {
    match state {
        "starting" | "running" => ToolStatus::Running,
        "ok" => ToolStatus::Ok,
        "error" => ToolStatus::Error,
        "stopped" => ToolStatus::Interrupted,
        _ => fallback,
    }
}

fn snapshot_sections(
    remaining: &[&str],
    metrics_index: Option<usize>,
    display: SnapshotDisplay,
) -> (Vec<String>, Vec<String>) {
    let mut summary = Vec::new();
    let mut result = Vec::new();
    let mut in_result = false;
    let mut status_continuation = false;

    for (index, line) in remaining.iter().copied().enumerate() {
        if Some(index) == metrics_index || line.starts_with("attach: ") {
            status_continuation = false;
            continue;
        }
        if matches!(display, SnapshotDisplay::Completion) && !in_result && line.is_empty() {
            in_result = true;
            continue;
        }
        if in_result {
            result.push(line.to_string());
            continue;
        }

        let formatted = if let Some(activity) = line.strip_prefix("activity: ") {
            status_continuation = true;
            format!("  {activity}")
        } else if let Some(latest) = line.strip_prefix("latest: ") {
            status_continuation = true;
            format!("  {latest}")
        } else if line == "completion result uses automatic delivery" {
            status_continuation = false;
            "  result will arrive automatically".into()
        } else if is_snapshot_protocol_line(line) {
            status_continuation = false;
            line.to_string()
        } else if matches!(display, SnapshotDisplay::Status) && status_continuation {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        } else {
            line.to_string()
        };
        summary.push(formatted);
    }
    (summary, result)
}

fn is_snapshot_protocol_line(line: &str) -> bool {
    line.starts_with("error: ")
        || line.starts_with("attachment error: ")
        || line == "this delegated task did not complete; treat its work as unverified"
}

fn turns_from_metrics(metrics: &str) -> Option<String> {
    let turns = metrics.split("turns: ").nth(1)?.split(" ·").next()?;
    Some(if turns == "1" {
        "1 turn".into()
    } else {
        format!("{turns} turns")
    })
}

fn elapsed_from_metrics(metrics: &str) -> Option<&str> {
    metrics.strip_prefix("elapsed: ")?.split(" ·").next()
}

fn tokens_from_metrics(metrics: &str) -> Option<&str> {
    metrics.split("tokens: ").nth(1)
}

fn state_glyph(state: &str) -> &'static str {
    match state {
        "starting" | "running" => "●",
        "ok" => "✓",
        "error" => "✗",
        "stopped" => "■",
        _ => "○",
    }
}

fn display_state(state: &str) -> &str {
    match state {
        "ok" => "completed",
        "error" => "failed",
        other => other,
    }
}

fn run_id_from_agent_line(line: &str) -> Option<&str> {
    strip_run_prefix(line)?.split_whitespace().next()
}

fn strip_run_prefix(line: &str) -> Option<&str> {
    line.strip_prefix("agent ")
        .or_else(|| line.strip_prefix("subagent "))
}

fn agent_identity(arguments: &serde_json::Value) -> Option<&str> {
    string_value(arguments, "agent_id").or_else(|| string_value(arguments, "preset"))
}

fn string_argument<'a>(view: &'a ToolView, key: &str) -> Option<&'a str> {
    string_value(&view.arguments, key)
}

fn string_value<'a>(arguments: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    arguments.get(key)?.as_str()
}

fn bool_value(arguments: &serde_json::Value, key: &str) -> bool {
    arguments
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
