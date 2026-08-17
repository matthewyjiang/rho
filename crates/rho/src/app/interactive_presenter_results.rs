//! Result-side ToolCard builders for interactive tool presentation.

use rho_tools::{
    parse_shell_content,
    tool_card::{
        compact_diff_rows, compact_diff_rows_from_card_files, parse_unified_diff, DiffCardChange,
        DiffCardFile, DiffRow, DiffRowKind, ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader,
        ToolStatus,
    },
};

use super::super::{ToolKind, ToolView};
use super::{
    display_path, draft_card, edit_paths, first_url, metadata_paths, quoted, search_terms,
    start_card, string_arg, truncate,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EmptyDiffState {
    Silent,
    NoChanges,
}

pub(super) fn shell_card(
    prompt: &str,
    arguments: &serde_json::Value,
    status: ToolStatus,
) -> ToolCard {
    let command = string_arg(arguments, "command").filter(|command| !command.trim().is_empty());
    let mut card = draft_card(
        status,
        ToolFamily::FileCommand,
        ToolHeader::shell(prompt, command),
    );
    push_shell_timeout_fact(&mut card, arguments);
    card
}

pub(super) fn shell_result_card(
    prompt: &str,
    arguments: &serde_json::Value,
    content: &str,
    status: ToolStatus,
) -> ToolCard {
    let command = string_arg(arguments, "command").filter(|command| !command.trim().is_empty());
    let mut card = draft_card(
        status,
        ToolFamily::FileCommand,
        ToolHeader::shell(prompt, command),
    );
    push_shell_timeout_fact(&mut card, arguments);

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
    } else if let Some(status) = parsed.exit_status {
        let text = match parsed.duration_ms {
            Some(ms) => format!("exit {status} · {:.1}s", ms as f64 / 1000.0),
            None => format!("exit {status}"),
        };
        card.push_fact(ToolFact::Meta { text });
    } else if parsed.running {
        card.push_fact(ToolFact::Meta {
            text: "running".into(),
        });
    } else if let Some(ms) = parsed.duration_ms {
        // Success omits `exit code: 0` on the wire; the card still shows timing.
        card.push_fact(ToolFact::Exit {
            code: 0,
            duration_ms: Some(ms),
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
    let arg_paths = if let ToolKind::Edit(format) = view.kind {
        edit_paths(format, &view.arguments, cwd)
    } else {
        let path = display_path(&view.arguments, cwd);
        if path.is_empty() {
            Vec::new()
        } else {
            vec![path]
        }
    };
    let paths = if paths.is_empty() { arg_paths } else { paths };
    if ok {
        let diff = view.metadata.unified_diff().unwrap_or(content);
        let omitted_diff_notices = diff
            .lines()
            .map(str::trim_end)
            .filter(|line| line.starts_with("Diff omitted:"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        let files = parse_unified_diff(diff)
            .into_iter()
            .map(DiffCardFile::from)
            .collect::<Vec<_>>();
        let has_no_changes =
            files.is_empty() && omitted_diff_notices.is_empty() && diff_reports_no_changes(diff);
        let mut card = diff_card(
            status,
            view.name.as_str(),
            paths,
            files,
            if has_no_changes {
                EmptyDiffState::NoChanges
            } else {
                EmptyDiffState::Silent
            },
            /*truncated*/ false,
        );
        for text in omitted_diff_notices {
            card.push_fact(ToolFact::Meta { text });
        }
        return card;
    }

    let mut card = diff_card(
        status,
        view.name.as_str(),
        paths,
        Vec::new(),
        EmptyDiffState::Silent,
        /*truncated*/ false,
    );
    if !content.trim().is_empty() {
        push_error_output(&mut card, content);
    }
    card
}

fn diff_reports_no_changes(diff: &str) -> bool {
    diff.lines().any(|line| line.trim_end() == "No changes.")
}

pub(super) fn diff_card(
    status: ToolStatus,
    name: &str,
    fallback_paths: Vec<String>,
    files: Vec<DiffCardFile>,
    empty_state: EmptyDiffState,
    truncated: bool,
) -> ToolCard {
    // Prefer paths from parsed content so deleted files keep their old path and
    // multi-file bodies get headings even when metadata is thin.
    let mut header_paths = files
        .iter()
        .map(DiffCardFile::display_path)
        .collect::<Vec<_>>();
    for path in fallback_paths {
        if !header_paths.contains(&path) {
            header_paths.push(path);
        }
    }
    let primary = match header_paths.as_slice() {
        [] => None,
        [path] => Some(path.clone()),
        paths => Some(format!("{} files", paths.len())),
    };
    let mut card = draft_card(
        status,
        ToolFamily::FileDiff,
        ToolHeader::call(name, primary),
    );
    if files.is_empty() {
        if empty_state == EmptyDiffState::NoChanges && !header_paths.is_empty() {
            card.push_fact(ToolFact::Meta {
                text: "no changes".into(),
            });
        }
        if truncated {
            card.body = ToolBody::Diff(vec![DiffRow::new(
                DiffRowKind::Skip,
                None,
                "⋯ more changes",
            )]);
        }
        return card;
    }

    // Path appears once: multi-file File section headers own path + counts;
    // single-file headers already name the path, so DiffStat is counts only.
    let include_file_headers = files.len() > 1;
    for file in &files {
        match file.change {
            DiffCardChange::Delete => {
                let text = if include_file_headers {
                    format!("delete {}", file.path)
                } else {
                    "delete".into()
                };
                card.push_fact(ToolFact::Meta { text });
            }
            DiffCardChange::Content => {
                // Multi-file: counts live on File section rows (see body below).
                if include_file_headers {
                    continue;
                }
                if let Some((added, removed)) = file.stats {
                    card.push_fact(ToolFact::DiffStat {
                        added,
                        removed,
                        path: None,
                    });
                }
            }
        }
    }
    let mut rows = compact_diff_rows_from_card_files(&files, include_file_headers);
    if truncated {
        rows.push(DiffRow::new(DiffRowKind::Skip, None, "⋯ more changes"));
    }
    if !rows.is_empty() {
        card.body = ToolBody::Diff(rows);
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
    // Grep content bodies paint language + match overlay from this pattern,
    // using the same literal/case semantics as the grep tool request.
    if view.kind == ToolKind::Grep {
        if let Some(pattern) = string_arg(&view.arguments, "pattern") {
            let literal = view
                .arguments
                .get("literal")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let case_sensitive = view
                .arguments
                .get("case_sensitive")
                .and_then(|value| value.as_bool())
                .unwrap_or(true);
            card = card
                .with_match_pattern(pattern)
                .with_match_semantics(literal, case_sensitive);
        }
    }
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
            let mut card = draft_card(
                status,
                ToolFamily::Default,
                ToolHeader::call("process", Some("stop".into())),
            );
            card.push_fact(ToolFact::Meta {
                text: format!("stop requested: {}", receipt.process_id),
            });
            return card;
        }
    }
    let Ok(snapshot) = serde_json::from_str::<crate::tools::process::Snapshot>(content) else {
        return compact_process_card(content, status);
    };

    let mut card = draft_card(
        status,
        ToolFamily::Default,
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

fn compact_process_card(content: &str, status: ToolStatus) -> ToolCard {
    let header = compact_header_block(content);
    if let Some(process_id) = header_value(header, "process_id") {
        if header.lines().any(|line| line == "stop requested") {
            let mut card = draft_card(
                status,
                ToolFamily::Default,
                ToolHeader::call("process", Some("stop".into())),
            );
            card.push_fact(ToolFact::Meta {
                text: format!("stop requested: {process_id}"),
            });
            return card;
        }
        let state = header_value(header, "state");
        let mut card = draft_card(
            status,
            ToolFamily::Default,
            ToolHeader::call("process", state.clone()),
        );
        let mut meta = process_id;
        if let Some(next) = header_value(header, "next") {
            meta.push_str(&format!(" · next {next}"));
        }
        if let Some(code) = header_value(header, "exit") {
            meta.push_str(&format!(" · exit {code}"));
        }
        card.push_fact(ToolFact::Meta { text: meta });
        if header.lines().any(|line| line == "pending") {
            card.push_fact(ToolFact::Meta {
                text: "more output available".into(),
            });
        }
        let body = stream_body_lines(content);
        if !body.is_empty() {
            card.body = ToolBody::Lines(body);
        }
        return card;
    }
    let mut card = draft_card(
        status,
        ToolFamily::Default,
        ToolHeader::call("process", None),
    );
    if !content.trim().is_empty() {
        card.body = ToolBody::Lines(split_body_lines(content));
    }
    card
}

fn header_value(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}: ");
    content.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn compact_header_block(content: &str) -> &str {
    content
        .split_once("\n\n")
        .map_or(content, |(header, _)| header)
}

fn compact_web_search_summary(content: &str) -> Option<String> {
    let mut lines = content.lines();
    if !lines
        .next()
        .is_some_and(|line| line.starts_with("responseId: "))
    {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join("\n");
    (!body.trim().is_empty()).then_some(body)
}

fn push_compact_fetch_fact(card: &mut ToolCard, content: &str) {
    if !content.starts_with("responseId: ") {
        card.push_fact(ToolFact::Meta {
            text: "finished".into(),
        });
        return;
    }
    let header = compact_header_block(content);
    let count = header_value(header, "items")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let truncated = header.lines().any(|line| line == "truncated");
    card.push_fact(ToolFact::Count {
        label: if count == 1 {
            "item".into()
        } else {
            "items".into()
        },
        value: count,
        detail: truncated.then(|| "truncated".into()),
    });
}

fn stream_body_lines(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut take = false;
    for line in content.lines() {
        if line == "stdout:" || line == "stderr:" {
            take = true;
            lines.push(line.to_string());
            continue;
        }
        if take {
            lines.push(line.to_string());
        }
    }
    lines
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
    let mut card = draft_card(
        status,
        ToolFamily::Web,
        ToolHeader::call("web_search", primary),
    );
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
        })
        .or_else(|| compact_web_search_summary(content));
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
    let mut card = draft_card(
        status,
        ToolFamily::Web,
        ToolHeader::call("fetch_content", primary),
    );
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            push_error_output(&mut card, content);
        }
        return card;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        push_compact_fetch_fact(&mut card, content);
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
    let mut card = draft_card(
        status,
        ToolFamily::Web,
        ToolHeader::call("get_search_content", None),
    );
    if status == ToolStatus::Error {
        if !content.trim().is_empty() {
            push_error_output(&mut card, content);
        }
        return card;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        if let Some(label) = header_value(content, "title")
            .or_else(|| header_value(content, "url"))
            .or_else(|| header_value(content, "query"))
        {
            card.header = ToolHeader::call("get_search_content", Some(truncate(&label, 80)));
        }
        card.push_fact(ToolFact::Meta {
            text: "retrieved".into(),
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
    let mut card = draft_card(
        status,
        ToolFamily::Default,
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
    content.lines().map(str::to_string).collect()
}

pub(super) fn count_nonempty_lines(content: &str) -> Option<u64> {
    let count = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count() as u64;
    (count > 0).then_some(count)
}

/// Timeout budget fact for shell cards. The TUI decorates it with a live
/// elapsed clock while the call runs (`timeout none · 1.2s`).
fn push_shell_timeout_fact(card: &mut ToolCard, arguments: &serde_json::Value) {
    let seconds = arguments
        .get("timeout_seconds")
        .and_then(|value| value.as_u64());
    card.push_fact(ToolFact::Timeout { seconds });
}

#[cfg(test)]
mod tests {
    use super::*;

    // Covers: fetch card facts come from the header block, not the inlined body
    // Owner: pure unit (presenter)
    #[test]
    fn compact_fetch_reads_only_header_fields() {
        let id = "0123456789abcdef0123456789abcdef";
        let cases = [
            (
                format!("responseId: {id}\nurl: https://example.com\n\n1. first\n2. second"),
                1_u64,
                None,
            ),
            (
                format!("responseId: {id}\nitems: 2\n0. https://a.example\n1. https://b.example"),
                2,
                None,
            ),
            (
                format!("responseId: {id}\nurl: https://example.com\n\nitems: 7\ntruncated"),
                1,
                None,
            ),
            (
                format!("responseId: {id}\nurl: https://example.com\ntruncated\n\nbody"),
                1,
                Some("truncated".into()),
            ),
        ];
        for (content, value, detail) in cases {
            let mut card = draft_card(
                ToolStatus::Ok,
                ToolFamily::Web,
                ToolHeader::call("fetch_content", None),
            );
            push_compact_fetch_fact(&mut card, &content);
            let label = if value == 1 { "item" } else { "items" };
            assert_eq!(
                card.facts,
                vec![ToolFact::Count {
                    label: label.into(),
                    value,
                    detail,
                }],
                "{content}"
            );
        }
    }

    // Covers: process stdout must not spoof pending / exit / stop header facts
    // Owner: pure unit (presenter)
    #[test]
    fn compact_process_ignores_header_tokens_in_streams() {
        let card = compact_process_card(
            "process_id: proc-1\nstate: running\nnext: 2\n\nstdout:\npending\nexit: 1\nstop requested",
            ToolStatus::Ok,
        );
        assert_eq!(
            card.header,
            ToolHeader::call("process", Some("running".into()))
        );
        assert_eq!(
            card.facts,
            vec![ToolFact::Meta {
                text: "proc-1 · next 2".into(),
            }]
        );
    }
}
