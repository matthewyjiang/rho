use super::ToolView;

const TASK_PREVIEW_BYTES: usize = 160;

pub(super) fn agent_start_lines_for(arguments: &serde_json::Value) -> Vec<String> {
    let agent_id = agent_identity(arguments).unwrap_or("agent");
    let background = bool_value(arguments, "background");
    let mode = if background {
        "starting in background"
    } else {
        "starting"
    };
    task_lines(arguments, format!("● {agent_id}  {mode}"))
}

pub(super) fn agent_interrupted_lines_for(arguments: &serde_json::Value) -> Vec<String> {
    let agent_id = agent_identity(arguments).unwrap_or("agent");
    task_lines(arguments, format!("■ {agent_id}  interrupted"))
}

pub(super) fn agents_interrupted_lines_for(arguments: &serde_json::Value) -> Vec<String> {
    let action = string_value(arguments, "action").unwrap_or("request");
    let heading = string_value(arguments, "id").map_or_else(
        || format!("■ delegated agents  {action} interrupted"),
        |id| format!("■ {id}  {action} interrupted"),
    );
    vec![heading]
}

pub(super) fn agent_progress_lines(view: &ToolView, content: &str) -> Vec<String> {
    let agent_id = agent_identity(&view.arguments).unwrap_or("agent");
    let mut lines = task_lines(&view.arguments, format!("● {agent_id}  running"));
    if let Some(run_id) = run_id_from_agent_line(content.lines().next().unwrap_or_default()) {
        lines.push(String::new());
        lines.push(format!("  {run_id} · rho attach {run_id}"));
    }
    lines
}

pub(super) fn agent_finished_lines(view: &ToolView, content: &str, ok: bool) -> Vec<String> {
    if let (true, Some(receipt)) = (ok, parse_background_receipt(content)) {
        let mut lines = task_lines(
            &view.arguments,
            format!("● {}  running in background", receipt.agent_id),
        );
        lines.push(String::new());
        lines.push(format!(
            "  {} · rho attach {}",
            receipt.run_id, receipt.run_id
        ));
        return lines;
    }
    if let Some(snapshot) = parse_snapshot(content) {
        return snapshot_lines(view, snapshot, SnapshotDisplay::Completion);
    }
    if !ok {
        let agent_id = agent_identity(&view.arguments).unwrap_or("agent");
        let mut lines = task_lines(&view.arguments, format!("✗ {agent_id}  failed"));
        push_content(&mut lines, content);
        return lines;
    }

    let agent_id = agent_identity(&view.arguments).unwrap_or("agent");
    let mut lines = task_lines(&view.arguments, format!("✓ {agent_id}  completed"));
    push_content(&mut lines, content);
    lines
}

pub(super) fn agents_start_lines_for(arguments: &serde_json::Value) -> Vec<String> {
    match string_value(arguments, "action") {
        Some("list") => vec!["● delegated agents  loading".into()],
        Some("status") => vec![format!(
            "● {}  checking status",
            string_value(arguments, "id").unwrap_or("delegated agent")
        )],
        Some("stop") => vec![format!(
            "● {}  stopping",
            string_value(arguments, "id").unwrap_or("delegated agent")
        )],
        Some(action) => vec![format!("● agents  {action}")],
        None => vec!["agents".into()],
    }
}

pub(super) fn agents_finished_lines(view: &ToolView, content: &str, ok: bool) -> Vec<String> {
    if !ok {
        let action = string_argument(view, "action").unwrap_or("request");
        let mut lines = vec![format!("✗ agents {action}  failed")];
        push_content(&mut lines, content);
        return lines;
    }

    match string_argument(view, "action") {
        Some("list") => agent_list_lines(content),
        Some(action @ ("status" | "stop")) => parse_snapshot(content)
            .map(|snapshot| {
                let display = if action == "status" || snapshot.has_status_metrics() {
                    SnapshotDisplay::Status
                } else {
                    SnapshotDisplay::Completion
                };
                snapshot_lines(view, snapshot, display)
            })
            .unwrap_or_else(|| agents_result_fallback_lines(view, content)),
        _ => {
            let mut lines = vec!["agents".into()];
            push_content(&mut lines, content);
            lines
        }
    }
}

fn agents_result_fallback_lines(view: &ToolView, content: &str) -> Vec<String> {
    let action = string_argument(view, "action").unwrap_or("request");
    let id = string_argument(view, "id");
    let heading = id.map_or_else(
        || format!("○ agents  {action} result"),
        |id| format!("○ {id}  {action} result"),
    );
    let mut lines = vec![heading];
    push_content(&mut lines, content);
    lines
}

fn task_lines(arguments: &serde_json::Value, heading: String) -> Vec<String> {
    let mut lines = vec![heading];
    if let Some(task) = string_value(arguments, "prompt") {
        let task = task.split_whitespace().collect::<Vec<_>>().join(" ");
        if !task.is_empty() {
            lines.push(format!("  {}", truncate_preview(&task)));
        }
    }
    lines
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

fn snapshot_lines(
    view: &ToolView,
    snapshot: Snapshot<'_>,
    display: SnapshotDisplay,
) -> Vec<String> {
    let metrics_index = snapshot
        .remaining
        .iter()
        .position(|line| line.starts_with("turns: ") || line.starts_with("elapsed: "));
    let metrics = metrics_index.map(|index| snapshot.remaining[index]);
    let turns = metrics.and_then(turns_from_metrics);
    let elapsed = metrics.and_then(elapsed_from_metrics);

    let mut details = Vec::new();
    if let Some(elapsed) = elapsed {
        details.push(elapsed.to_string());
    }
    if let Some(turns) = turns {
        details.push(turns);
    }
    let detail = if details.is_empty() {
        String::new()
    } else {
        format!(" · {}", details.join(" · "))
    };
    let mut lines = task_lines(
        &view.arguments,
        format!(
            "{} {}  {}{}",
            state_glyph(snapshot.state),
            snapshot.agent_id,
            display_state(snapshot.state),
            detail
        ),
    );

    let tokens = metrics.and_then(tokens_from_metrics);
    let attach = snapshot
        .remaining
        .iter()
        .find_map(|line| line.strip_prefix("attach: "));
    let (summary_lines, result_lines) =
        snapshot_sections(&snapshot.remaining, metrics_index, display);
    lines.extend(summary_lines);

    if tokens.is_some() || attach.is_some() || !snapshot.run_id.is_empty() {
        lines.push(String::new());
        lines.push(match (tokens, attach) {
            (Some(tokens), _) => format!("  {} · {tokens}", snapshot.run_id),
            (None, Some(attach)) => format!("  {} · {attach}", snapshot.run_id),
            (None, None) => format!("  {}", snapshot.run_id),
        });
        if tokens.is_some() {
            if let Some(attach) = attach {
                lines.push(format!("  {attach}"));
            }
        }
    }
    if !result_lines.is_empty() {
        lines.push(String::new());
        lines.extend(result_lines);
    }
    lines
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

fn push_content(lines: &mut Vec<String>, content: &str) {
    if !content.trim().is_empty() {
        lines.push(String::new());
        lines.push(content.to_string());
    }
}
