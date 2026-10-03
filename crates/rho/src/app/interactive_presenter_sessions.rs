//! Sessions owns the evidence-to-card projection; renderers only know receipts.

use rho_tools::tool_card::{ToolBody, ToolCard, ToolFact, ToolFamily, ToolHeader, ToolStatus};
use serde::Deserialize;
use serde_json::Value;

use super::format::string_arg;

pub(super) fn preview_card(arguments: &Value, status: ToolStatus) -> ToolCard {
    let (verb, primary) = match arguments.get("action").and_then(Value::as_str) {
        Some("search") => (
            "sessions.search",
            string_arg(arguments, "query").map(|q| format!("{q:?}")),
        ),
        Some("read") => (
            "sessions.read",
            // Match the eight-character session identity used by the sessions CLI.
            string_arg(arguments, "session").map(|id| id.chars().take(8).collect()),
        ),
        Some("recall") => ("sessions.recall", string_arg(arguments, "recall_id")),
        _ => ("sessions", None),
    };
    ToolCard::new(status, ToolFamily::Default, ToolHeader::call(verb, primary))
}

/// Unknown or malformed payloads use the normal visible-output fallback.
/// Errors must never disappear into a collapsed evidence body.
pub(super) fn finished_card(arguments: &Value, content: &str, ok: bool) -> Option<ToolCard> {
    if !ok {
        return None;
    }
    let mut card = preview_card(arguments, ToolStatus::Ok);
    let mut lines = Vec::new();
    let (summary, index) = match arguments.get("action")?.as_str()? {
        "search" => search_receipt(serde_json::from_str(content).ok()?, &mut lines),
        "read" => {
            let result: ReadResult = serde_json::from_str(content).ok()?;
            let excerpt = result.excerpt;
            let mut summary = format!(
                "{} · chars {}–{} of {}",
                excerpt.role, excerpt.start, excerpt.end, excerpt.total_chars
            );
            if result.next_start.is_some() {
                summary.push_str(" · more available");
            }
            if excerpt.omitted_blocks > 0 {
                summary.push_str(&format!(" · {} blocks omitted", excerpt.omitted_blocks));
            }
            lines.push(format!("session: {}", result.session));
            lines.push(format!("anchor: {}", excerpt.anchor));
            lines.push(String::new());
            lines.extend(excerpt.text.lines().map(str::to_owned));
            if let Some(start) = result.next_start {
                lines.push(format!("passage continues at char {start}"));
            }
            if let Some(anchor) = result.next_anchor {
                lines.push(format!("next anchor: {anchor}"));
            }
            if let Some(anchor) = result.previous_anchor {
                lines.push(format!("previous anchor: {anchor}"));
            }
            (summary, result.index)
        }
        _ => return None,
    };
    card.push_fact(ToolFact::Meta { text: summary });
    if index.skipped_files > 0 || index.omitted_records > 0 {
        card.push_fact(ToolFact::Meta {
            text: format!(
                "index omissions: {} files skipped · {} records omitted",
                index.skipped_files, index.omitted_records
            ),
        });
    }
    card.body = ToolBody::Lines(lines);
    Some(card)
}

/// Builds the receipt summary and fills the expanded body for one search page.
fn search_receipt(page: SearchResult, lines: &mut Vec<String>) -> (String, IndexReport) {
    let mut body = Vec::new();
    let (count, total, noun) = match page.listing {
        SearchListing::Sessions {
            total_sessions,
            sessions,
        } => {
            let count = sessions.len();
            for group in sessions {
                body.push(String::new());
                body.push(format!("session {} · {}", group.id, group.workspace));
                body.push(format!("handle: {}", group.session));
                body.push(format!(
                    "{} matching {}",
                    group.matching_messages,
                    plural(group.matching_messages, "message")
                ));
                for excerpt in group.excerpts {
                    push_excerpt(&mut body, excerpt);
                }
                if group.omitted_matches > 0 {
                    body.push(format!(
                        "{} more matching messages not included",
                        group.omitted_matches
                    ));
                }
            }
            (count, total_sessions, "session")
        }
        SearchListing::Matches {
            session,
            total_matches,
            matches,
        } => {
            let count = matches.len();
            if let Some(session) = session {
                body.push(String::new());
                body.push(format!("handle: {session}"));
            }
            for excerpt in matches {
                push_excerpt(&mut body, excerpt);
            }
            (count, total_matches, "message")
        }
    };
    let noun = plural(total, noun);
    let mut summary = if total == 0 {
        format!("no matching {noun} · {}", page.scope)
    } else if count == total {
        format!("{count} matching {noun} · {}", page.scope)
    } else {
        format!("{count} of {total} matching {noun} · {}", page.scope)
    };
    if let Some(offset) = page.next_offset {
        summary.push_str(" · more results available");
        lines.push(format!("next search offset: {offset}"));
    }
    if let Some(budget) = page.output_budget_bytes {
        summary.push_str(&format!(" · output limited to {budget} bytes"));
    }
    // Blank lines separate sections, so none leads an otherwise empty body.
    let skip = usize::from(lines.is_empty() && body.first().is_some_and(String::is_empty));
    lines.extend(body.into_iter().skip(skip));
    (summary, page.index)
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_owned()
    } else {
        format!("{noun}s")
    }
}

fn push_excerpt(lines: &mut Vec<String>, excerpt: Excerpt) {
    lines.push(String::new());
    lines.push(format!(
        "{} · {} · chars {}–{} of {}",
        excerpt.role, excerpt.anchor, excerpt.start, excerpt.end, excerpt.total_chars
    ));
    lines.extend(excerpt.text.lines().map(str::to_owned));
    if excerpt.omitted_blocks > 0 {
        lines.push(format!("{} blocks omitted", excerpt.omitted_blocks));
    }
}

#[derive(Deserialize)]
struct SearchResult {
    index: IndexReport,
    scope: String,
    next_offset: Option<usize>,
    output_budget_bytes: Option<usize>,
    #[serde(flatten)]
    listing: SearchListing,
}

/// Prior-session scopes list session groups; scope `current` lists messages.
#[derive(Deserialize)]
#[serde(untagged)]
enum SearchListing {
    Sessions {
        total_sessions: usize,
        sessions: Vec<SessionGroup>,
    },
    Matches {
        session: Option<String>,
        total_matches: usize,
        matches: Vec<Excerpt>,
    },
}

#[derive(Deserialize)]
struct SessionGroup {
    session: String,
    id: String,
    workspace: String,
    matching_messages: usize,
    excerpts: Vec<Excerpt>,
    omitted_matches: usize,
}

#[derive(Deserialize)]
struct Excerpt {
    anchor: String,
    role: String,
    start: usize,
    end: usize,
    total_chars: usize,
    text: String,
    omitted_blocks: usize,
}

#[derive(Deserialize)]
struct ReadResult {
    index: IndexReport,
    session: String,
    #[serde(flatten)]
    excerpt: Excerpt,
    next_start: Option<usize>,
    next_anchor: Option<String>,
    previous_anchor: Option<String>,
}

/// Keep incomplete-index notices visible, but omit routine cache statistics.
#[derive(Deserialize)]
struct IndexReport {
    skipped_files: usize,
    omitted_records: usize,
}
