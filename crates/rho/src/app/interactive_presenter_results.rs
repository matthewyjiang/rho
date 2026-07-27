//! Result-side ToolCard builders for interactive tool presentation.

use rho_tools::tool_card::{
    compact_diff_lines, diff_file_stats, parse_shell_content, ToolBody, ToolCard, ToolFact,
    ToolFamily, ToolHeader, ToolStatus,
};

use super::super::{ToolKind, ToolView};
use super::{
    display_path, edit_paths, family_for_kind, first_url, metadata_paths, quoted, search_terms,
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
            ToolCard::new(
                status,
                ToolFamily::FileCommand,
                ToolHeader::shell(prompt, command),
            )
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
    let mut card = ToolCard::new(
        status,
        ToolFamily::FileCommand,
        ToolHeader::shell(prompt, command),
    );
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
            card.push_fact(ToolFact::Error { text: notice });
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
    let mut card = ToolCard::new(
        status,
        ToolFamily::FileCommand,
        ToolHeader::call(view.name.as_str(), primary),
    );
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
        let compact = compact_diff_lines(diff, paths.len() > 1);
        if !compact.is_empty() {
            card.body = ToolBody::DiffLines(compact);
        }
    } else if !content.trim().is_empty() {
        card.push_fact(ToolFact::Error {
            text: content.trim().to_string(),
        });
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
            card.push_fact(ToolFact::Error {
                text: content.trim().to_string(),
            });
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
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        let mut card = ToolCard::new(
            status,
            ToolFamily::Default,
            ToolHeader::call("process", None),
        );
        if !content.trim().is_empty() {
            card.body = ToolBody::Lines(split_body_lines(content));
        }
        return card;
    };
    if value
        .get("stop_requested")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        let mut card = ToolCard::new(
            status,
            ToolFamily::Default,
            ToolHeader::call("process", Some("stop".into())),
        );
        if let Some(id) = value.get("process_id").and_then(|value| value.as_str()) {
            card.push_fact(ToolFact::Meta {
                text: format!("stop requested: {id}"),
            });
        }
        return card;
    }
    let action = value
        .get("state")
        .and_then(|value| value.as_str())
        .map(|state| state.replace('_', " "));
    let mut card = ToolCard::new(
        status,
        ToolFamily::Default,
        ToolHeader::call("process", action),
    );
    if let Some(command) = value.get("command").and_then(|value| value.as_str()) {
        card.push_fact(ToolFact::Text {
            text: command.to_string(),
        });
    }
    let id = value
        .get("process_id")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let runtime = value
        .get("runtime_seconds")
        .and_then(|value| value.as_f64())
        .unwrap_or_default();
    let mut meta = format!("{id} · {runtime:.1}s");
    if let Some(code) = value.get("exit_code").and_then(|value| value.as_i64()) {
        meta.push_str(&format!(" · exit {code}"));
    }
    card.push_fact(ToolFact::Meta { text: meta });
    if value.get("truncated").and_then(|value| value.as_bool()) == Some(true) {
        let cursor = value
            .get("first_cursor")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        card.push_fact(ToolFact::Meta {
            text: format!("output before cursor {cursor} is no longer available"),
        });
    }
    let mut body = Vec::new();
    if let Some(chunks) = value.get("chunks").and_then(|value| value.as_array()) {
        for chunk in chunks {
            let stream = chunk
                .get("stream")
                .and_then(|value| value.as_str())
                .unwrap_or("stdout");
            body.push(format!("{stream}:"));
            body.push(
                chunk
                    .get("text")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    if value
        .get("output_pending")
        .and_then(|value| value.as_bool())
        == Some(true)
    {
        let cursor = value
            .get("next_cursor")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        card.push_fact(ToolFact::Meta {
            text: format!("more output available at cursor {cursor}"),
        });
    }
    if let Some(detail) = value
        .get("terminal_detail")
        .and_then(|value| value.as_str())
    {
        card.push_fact(ToolFact::Meta {
            text: format!("detail: {detail}"),
        });
    }
    if !body.is_empty() {
        card.body = ToolBody::Lines(body);
    }
    card
}

pub(super) fn web_search_card(
    arguments: &serde_json::Value,
    content: &str,
    status: ToolStatus,
) -> ToolCard {
    let primary = search_terms(arguments);
    let mut card = ToolCard::new(
        status,
        ToolFamily::Web,
        ToolHeader::call("web_search", primary),
    );
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            card.push_fact(ToolFact::Error {
                text: content.trim().to_string(),
            });
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
    let mut card = ToolCard::new(
        status,
        ToolFamily::Web,
        ToolHeader::call("fetch_content", primary),
    );
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            card.push_fact(ToolFact::Error {
                text: content.trim().to_string(),
            });
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
    let mut card = ToolCard::new(
        status,
        ToolFamily::Web,
        ToolHeader::call("get_search_content", None),
    );
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            card.push_fact(ToolFact::Error {
                text: content.trim().to_string(),
            });
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
    let mut card = ToolCard::new(
        status,
        family_for_kind(view.kind),
        ToolHeader::call(view.name.as_str(), None),
    );
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
        let compact = compact_diff_lines(diff, true);
        if !compact.is_empty() {
            card.body = ToolBody::DiffLines(compact);
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
            card.push_fact(ToolFact::Error {
                text: content.trim().to_string(),
            });
        } else if card.body.is_empty() {
            card.body = ToolBody::Lines(split_body_lines(content));
        }
    }
    card
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
