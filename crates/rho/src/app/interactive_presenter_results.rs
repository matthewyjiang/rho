//! Result-side ToolCard builders for interactive tool presentation.

use rho_tools::{
    parse_shell_content,
    tool_card::{
        compact_diff_rows, diff_file_stats, ToolBody, ToolCard, ToolFact, ToolHeader, ToolStatus,
    },
};

use super::super::{ToolKind, ToolView};
use super::{
    display_path, draft_card, edit_paths, first_url, metadata_paths, quoted, search_terms,
    start_card, string_arg, truncate,
};

pub(super) fn shell_card(
    prompt: &str,
    arguments: &serde_json::Value,
    status: ToolStatus,
    content: Option<&str>,
) -> ToolCard {
    match content {
        Some(content) => shell_result_card(prompt, arguments, content, status),
        None => {
            let command =
                string_arg(arguments, "command").filter(|command| !command.trim().is_empty());
            draft_card(status, ToolHeader::shell(prompt, command))
        }
    }
}

pub(super) fn shell_result_card(
    prompt: &str,
    arguments: &serde_json::Value,
    content: &str,
    status: ToolStatus,
) -> ToolCard {
    let command = string_arg(arguments, "command").filter(|command| !command.trim().is_empty());
    let mut card = draft_card(status, ToolHeader::shell(prompt, command));
    if let Some(seconds) = arguments
        .get("timeout_seconds")
        .and_then(|value| value.as_u64())
    {
        card.push_fact(ToolFact::Meta {
            text: format!("timeout {seconds}s"),
        });
    } else {
        card.push_fact(ToolFact::Meta {
            text: "timeout none".into(),
        });
    }

    let parsed = parse_shell_content(content);
    let notice = parsed
        .notice
        .as_deref()
        .map(str::trim)
        .filter(|notice| !notice.is_empty())
        .map(str::to_string);
    if let Some(notice) = notice.clone() {
        if status == ToolStatus::Error || notice.contains("timed out") {
            push_error_output(&mut card, &notice);
        } else {
            card.push_fact(ToolFact::Meta { text: notice });
        }
    }
    if let Some(code) = parsed.exit_code {
        card.push_fact(ToolFact::Exit {
            code,
            duration_ms: parsed.duration_ms,
        });
    } else if parsed.running {
        card.push_fact(ToolFact::Meta {
            text: "running".into(),
        });
    }
    if !parsed.stdout.trim().is_empty() {
        card.body = ToolBody::Lines(split_body_lines(&parsed.stdout));
    } else if !content.trim().is_empty()
        && notice.is_none()
        && parsed.exit_code.is_none()
        && !parsed.running
    {
        card.body = ToolBody::Lines(split_body_lines(content));
    }
    card
}

pub(super) fn file_diff_card(
    view: &ToolView,
    content: &str,
    ok: bool,
    cwd: &std::path::Path,
) -> ToolCard {
    let status = ToolStatus::from_finished(ok);
    let paths = metadata_paths(view, cwd);
    let arg_paths = if view.kind == ToolKind::EditFile {
        edit_paths(&view.arguments, cwd)
    } else {
        let path = display_path(&view.arguments, cwd);
        if path.is_empty() {
            Vec::new()
        } else {
            vec![path]
        }
    };
    let paths = if paths.is_empty() { arg_paths } else { paths };
    let primary = match paths.as_slice() {
        [] => None,
        [path] => Some(path.clone()),
        paths => Some(format!("{} files", paths.len())),
    };
    let mut card = draft_card(status, ToolHeader::call(view.name.as_str(), primary));
    if ok {
        let diff = view.metadata.unified_diff().unwrap_or(content);
        let stats = diff_file_stats(diff);
        if stats.is_empty() {
            if paths.len() == 1 {
                card.push_fact(ToolFact::Meta {
                    text: "no changes".into(),
                });
            }
        } else {
            for stat in &stats {
                card.push_fact(ToolFact::DiffStat {
                    added: stat.added,
                    removed: stat.removed,
                    path: Some(stat.path.clone()),
                });
            }
        }
        let rows = compact_diff_rows(diff, paths.len() > 1);
        if !rows.is_empty() {
            card.body = ToolBody::Diff(rows);
        }
    } else if !content.trim().is_empty() {
        push_error_output(&mut card, content);
    }
    card
}

pub(super) fn search_result_card(
    view: &ToolView,
    content: &str,
    ok: bool,
    cwd: &std::path::Path,
) -> ToolCard {
    let status = ToolStatus::from_finished(ok);
    let mut card = start_card(view, cwd);
    card.status = status;
    if !ok {
        if !content.trim().is_empty() {
            push_error_output(&mut card, content);
        }
        return card;
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        card.push_fact(ToolFact::Count {
            label: "matches".into(),
            value: 0,
            detail: None,
        });
        return card;
    }
    let match_lines = trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    let file_count = trimmed
        .lines()
        .filter_map(|line| line.split_once(':').map(|(path, _)| path))
        .collect::<std::collections::BTreeSet<_>>()
        .len() as u64;
    if view.kind == ToolKind::Grep && file_count > 0 {
        card.push_fact(ToolFact::Count {
            label: if match_lines == 1 {
                "match".into()
            } else {
                "matches".into()
            },
            value: match_lines,
            detail: Some(format!(
                "in {} {}",
                file_count,
                if file_count == 1 { "file" } else { "files" }
            )),
        });
    } else {
        card.push_fact(ToolFact::Count {
            label: if match_lines == 1 {
                "match".into()
            } else {
                "matches".into()
            },
            value: match_lines,
            detail: None,
        });
    }
    card.body = ToolBody::Lines(split_body_lines(content));
    card
}

pub(super) fn process_result_card(content: &str, status: ToolStatus) -> ToolCard {
    #[derive(serde::Deserialize)]
    struct StopReceipt {
        stop_requested: bool,
        process_id: String,
    }

    if let Ok(receipt) = serde_json::from_str::<StopReceipt>(content) {
        if receipt.stop_requested {
            let mut card = draft_card(status, ToolHeader::call("process", Some("stop".into())));
            card.push_fact(ToolFact::Meta {
                text: format!("stop requested: {}", receipt.process_id),
            });
            return card;
        }
    }
    let Ok(snapshot) = serde_json::from_str::<crate::tools::process::Snapshot>(content) else {
        let mut card = draft_card(status, ToolHeader::call("process", None));
        if !content.trim().is_empty() {
            card.body = ToolBody::Lines(split_body_lines(content));
        }
        return card;
    };

    let mut card = draft_card(
        status,
        ToolHeader::call("process", Some(process_state(snapshot.state).into())),
    );
    card.push_fact(ToolFact::Text {
        text: snapshot.command,
    });
    let mut meta = format!("{} · {:.1}s", snapshot.process_id, snapshot.runtime_seconds);
    if let Some(code) = snapshot.exit_code {
        meta.push_str(&format!(" · exit {code}"));
    }
    card.push_fact(ToolFact::Meta { text: meta });
    if snapshot.truncated {
        card.push_fact(ToolFact::Meta {
            text: format!(
                "output before cursor {} is no longer available",
                snapshot.first_cursor
            ),
        });
    }
    let mut body = Vec::new();
    for chunk in snapshot.chunks {
        let stream = match chunk.stream {
            crate::tools::process::Stream::Stdout => "stdout",
            crate::tools::process::Stream::Stderr => "stderr",
        };
        body.push(format!("{stream}:"));
        body.push(chunk.text);
    }
    if snapshot.output_pending {
        card.push_fact(ToolFact::Meta {
            text: format!("more output available at cursor {}", snapshot.next_cursor),
        });
    }
    if let Some(detail) = snapshot.terminal_detail {
        card.push_fact(ToolFact::Meta {
            text: format!("detail: {detail}"),
        });
    }
    if !body.is_empty() {
        card.body = ToolBody::Lines(body);
    }
    card
}

fn process_state(state: crate::tools::process::State) -> &'static str {
    use crate::tools::process::State;
    match state {
        State::Starting => "starting",
        State::Running => "running",
        State::Exited => "exited",
        State::Terminated => "terminated",
        State::TimedOut => "timed out",
        State::FailedToStart => "failed to start",
    }
}

pub(super) fn web_search_card(
    arguments: &serde_json::Value,
    content: &str,
    status: ToolStatus,
) -> ToolCard {
    let primary = search_terms(arguments);
    let mut card = draft_card(status, ToolHeader::call("web_search", primary));
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            push_error_output(&mut card, content);
        }
        return card;
    }
    let summary = serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            value
                .get("answer")
                .and_then(|answer| answer.as_str())
                .map(str::to_string)
        });
    match summary.as_deref() {
        Some(answer) if answer.starts_with("No configured search provider") => {
            card.push_fact(ToolFact::Meta {
                text: "no live results".into(),
            });
        }
        Some(answer) => {
            let count = answer
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u64;
            card.push_fact(ToolFact::Count {
                label: if count == 1 {
                    "result".into()
                } else {
                    "results".into()
                },
                value: count,
                detail: Some("stored".into()),
            });
        }
        None => card.push_fact(ToolFact::Meta {
            text: "finished".into(),
        }),
    }
    card
}

pub(super) fn fetch_content_card(
    arguments: &serde_json::Value,
    content: &str,
    status: ToolStatus,
) -> ToolCard {
    let primary = first_url(arguments);
    let mut card = draft_card(status, ToolHeader::call("fetch_content", primary));
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            push_error_output(&mut card, content);
        }
        return card;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        card.push_fact(ToolFact::Meta {
            text: "finished".into(),
        });
        return card;
    };
    if let Some(count) = value.get("itemCount").and_then(|count| count.as_u64()) {
        let truncated = value
            .get("contentTruncated")
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false);
        let detail = if truncated {
            Some("truncated".into())
        } else {
            None
        };
        card.push_fact(ToolFact::Count {
            label: if count == 1 {
                "item".into()
            } else {
                "items".into()
            },
            value: count,
            detail,
        });
        return card;
    }
    if let Some(items) = value.get("items").and_then(|items| items.as_array()) {
        card.push_fact(ToolFact::Count {
            label: if items.len() == 1 {
                "item".into()
            } else {
                "items".into()
            },
            value: items.len() as u64,
            detail: None,
        });
        return card;
    }
    if value.get("content").is_some() {
        let truncated = value
            .get("contentTruncated")
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false);
        card.push_fact(ToolFact::Count {
            label: "item".into(),
            value: 1,
            detail: truncated.then(|| "truncated".into()),
        });
        return card;
    }
    card.push_fact(ToolFact::Meta {
        text: "finished".into(),
    });
    card
}

pub(super) fn get_search_content_card(content: &str, status: ToolStatus) -> ToolCard {
    let mut card = draft_card(status, ToolHeader::call("get_search_content", None));
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            push_error_output(&mut card, content);
        }
        return card;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        card.push_fact(ToolFact::Meta {
            text: "retrieved stored content".into(),
        });
        return card;
    };
    if let Some(query) = value.get("query").and_then(|value| value.as_str()) {
        card.header = ToolHeader::call("get_search_content", Some(quoted(query, 80)));
        card.push_fact(ToolFact::Meta {
            text: "retrieved".into(),
        });
        return card;
    }
    let label = value
        .get("title")
        .and_then(|value| value.as_str())
        .or_else(|| value.get("url").and_then(|value| value.as_str()))
        .map(|value| truncate(value, 80))
        .unwrap_or_else(|| "stored content".into());
    card.header = ToolHeader::call("get_search_content", Some(label));
    card.push_fact(ToolFact::Meta {
        text: "retrieved".into(),
    });
    card
}

pub(super) fn generic_card(view: &ToolView, content: &str, status: ToolStatus) -> ToolCard {
    let mut card = draft_card(status, ToolHeader::call(view.name.as_str(), None));
    if let Some(command) = view.metadata.command_summary_text() {
        card.push_fact(ToolFact::Text {
            text: command.to_string(),
        });
    }
    for path in view.metadata.affected_paths() {
        card.push_fact(ToolFact::Meta {
            text: path.display().to_string(),
        });
    }
    for url in view.metadata.urls() {
        card.push_fact(ToolFact::Meta { text: url.clone() });
    }
    if let Some(diff) = view.metadata.unified_diff() {
        let rows = compact_diff_rows(diff, true);
        if !rows.is_empty() {
            card.body = ToolBody::Diff(rows);
        }
    }
    if card.facts.is_empty()
        && card.body.is_empty()
        && view.arguments != serde_json::Value::Object(Default::default())
    {
        card.push_fact(ToolFact::Text {
            text: view.arguments.to_string(),
        });
    }
    if !content.trim().is_empty() {
        if status == ToolStatus::Error {
            push_error_output(&mut card, content);
        } else if card.body.is_empty() {
            card.body = ToolBody::Lines(split_body_lines(content));
        }
    }
    card
}

pub(super) fn push_error_output(card: &mut ToolCard, content: &str) {
    let lines = split_body_lines(content.trim());
    let Some(first) = lines.first() else {
        return;
    };
    let summary = truncate(first.trim(), 160);
    card.push_fact(ToolFact::Error {
        text: summary.clone(),
    });
    let detail = if summary == first.trim() {
        &lines[1..]
    } else {
        lines.as_slice()
    };
    if !detail.is_empty() {
        card.body = ToolBody::Lines(detail.to_vec());
    }
}

pub(super) fn split_body_lines(content: &str) -> Vec<String> {
    let lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    if lines.is_empty() {
        if content.is_empty() {
            Vec::new()
        } else {
            vec![content.to_string()]
        }
    } else {
        lines
    }
}

pub(super) fn count_nonempty_lines(content: &str) -> Option<u64> {
    let count = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    (count > 0).then_some(count)
}
