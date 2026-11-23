//! News on watched pull requests as the model reads it ([`prompt`]) and as
//! clients show it ([`News::card`]).

use rho_protocol::display::{
    NotificationCard, NotificationDelivery, NotificationPreview, NotificationTone,
    NotificationVisibility,
};

use super::{
    record::NEWS_PER_SESSION,
    state::{Change, CheckRef, CheckTally, End, Failure, Gate, News, Remark, Reviewed},
    watch::{COMMENT_WAKE_LIMIT, READ_FAILURE_LIMIT},
};
use crate::tools::notification_format::join_budgeted_sections;

/// The most one wake's text may take: the budget process and subagent
/// notifications have (`tools/process/notify.rs`, `tools/agent_output.rs`).
pub(crate) const MODEL_NOTIFICATION_BYTES: usize = 16 * 1024;

/// Items one change lists before `and N more`, and characters of a comment
/// it quotes: t3's `LISTED_ITEMS` and `SNIPPET_LENGTH`
/// (`pullRequestWatch.ts:14-15`).
const LISTED_ITEMS: usize = 10;
const SNIPPET_CHARS: usize = 200;

/// What a wake ends with while any watch it tells of is still on. Short,
/// since every wake repeats it; the no-polling line stays because agents
/// otherwise wait in the shell (t3's `evals/mcp-tool-use` cases).
const CLOSING: &str = "The watch stays on. Act on what needs it, then end your turn; rho wakes you on the next change. Don't watch again or poll (no sleep, `gh pr checks --watch` or background waits). Call unwatch_pull_request before you hand the work back.";

/// The text of one wake for a session's queued `news`, oldest first: each
/// pull request's changes and end, then [`CLOSING`] once if any watch is
/// still on. `dropped` counts older news the queue let go (see
/// [`NEWS_PER_SESSION`]); the text opens by saying so when it isn't 0. The
/// text fits [`MODEL_NOTIFICATION_BYTES`], and says how many updates it
/// left out to fit.
pub(crate) fn prompt(news: &[News], dropped: u32) -> String {
    let preface = if dropped == 0 {
        String::new()
    } else {
        let updates = plural(dropped as usize, "update", "updates");
        format!(
            "rho dropped {dropped} older {updates} for this session (it keeps {NEWS_PER_SESSION}). Read each pull request's current state.\n\n"
        )
    };
    let closing = if news.iter().any(|news| !news.is_end()) {
        format!("\n\n{CLOSING}")
    } else {
        String::new()
    };
    let budget = MODEL_NOTIFICATION_BYTES.saturating_sub(preface.len() + closing.len());
    let body = join_budgeted_sections(news.iter().map(section), "\n\n", budget, omitted);
    format!("{preface}{body}{closing}")
}

/// What [`prompt`] says in place of the last `count` updates when they
/// don't fit.
fn omitted(count: usize) -> String {
    let updates = plural(count, "update", "updates");
    format!(
        "... {count} more pull request {updates} left out to fit {} KiB; read their current state with gh.",
        MODEL_NOTIFICATION_BYTES / 1024
    )
}

/// One pull request's part of a wake.
fn section(news: &News) -> String {
    let label = news.pr.to_string();
    let mut lines = Vec::new();
    if !news.changes.is_empty() {
        lines.push(format!("Update on {label} ({}):", news.url));
        lines.extend(change_lines(news));
    }
    if let Some(end) = &news.end {
        lines.push(end_line(end, &label, &news.url));
    }
    lines.join("\n")
}

impl News {
    /// Transcript card for this news: what happened and which pull request
    /// up front, the head beneath, the same lines the model reads as the
    /// body. The tone is that of its most pressing item.
    pub(crate) fn card(&self) -> NotificationCard {
        let label = self.pr.to_string();
        let mut outcomes: Vec<&str> = self.changes.iter().map(change_summary).collect();
        outcomes.extend(self.end.as_ref().map(end_summary));
        let tone = self
            .changes
            .iter()
            .map(change_tone)
            .chain(self.end.iter().map(end_tone))
            .max_by_key(|tone| urgency(*tone))
            .unwrap_or(NotificationTone::Neutral);
        let mut body = change_lines(self);
        body.extend(
            self.end
                .as_ref()
                .map(|end| end_line(end, &label, &self.url)),
        );
        let mut details = vec![format!("pull request: {}", self.url)];
        details.extend(self.head.as_ref().map(|head| format!("head: {head}")));
        NotificationCard {
            title: format!("{} · {label}", capitalized(&outcomes.join(", "))),
            sender: "github".into(),
            recipient: "session".into(),
            delivery: NotificationDelivery::Received,
            tone,
            preview: NotificationPreview::Truncated,
            visibility: NotificationVisibility::Conversation,
            reference: Some(label),
            subtitle: self
                .head
                .as_deref()
                .map(|head| format!("head {}", short(head))),
            body: body.join("\n"),
            output: None,
            details,
        }
    }
}

/// A commit's first seven characters, as GitHub shows it.
fn short(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 {
        one
    } else {
        many
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The lines of `news`'s changes, as a Markdown list.
fn change_lines(news: &News) -> Vec<String> {
    let commit = news
        .head
        .as_deref()
        .map(|head| format!(" on {}", short(head)))
        .unwrap_or_default();
    let mut lines = Vec::new();
    if news.pushed {
        let head = news.head.as_deref().map_or("", short);
        lines.push(format!(
            "- New commits: the head is now {head}, so earlier check results no longer apply."
        ));
    }
    for change in &news.changes {
        match change {
            Change::ChecksFailed { failed } => {
                lines.push(format!("- Checks failed{commit}:"));
                lines.extend(listed(failed, check_line));
                lines.extend(news.checks.as_ref().map(tally_line));
            }
            Change::ChecksPassed { count, gate } => {
                let required = match gate {
                    Gate::Required => "required ",
                    Gate::All => "",
                };
                let checks = plural(*count as usize, "check", "checks");
                lines.push(format!("- All {count} {required}{checks} passed{commit}."));
            }
            Change::ChecksFinished { passed, failing } if failing.is_empty() => {
                lines.push(format!(
                    "- Every check has finished{commit}: {passed} passed, none failing."
                ));
            }
            Change::ChecksFinished { passed, failing } => {
                lines.push(format!(
                    "- Every check has finished{commit}: {passed} passed, {} still failing:",
                    failing.len()
                ));
                lines.extend(listed(failing, check_line));
            }
            Change::Remarks { remarks } => {
                let comments = plural(remarks.len(), "comment", "comments");
                lines.push(format!("- {} new {comments}:", remarks.len()));
                lines.extend(listed(remarks, remark_line));
            }
            Change::Conflicting => {
                lines.push("- The branch now conflicts with its base branch.".to_owned());
            }
        }
    }
    lines
}

/// `- Checks now: 10 passed, 2 failed, 2 running.`: where CI stands as
/// failures are told, so the agent can tell acting now from waiting.
fn tally_line(tally: &CheckTally) -> String {
    let CheckTally {
        passed,
        running,
        failed,
        unread,
    } = *tally;
    let unread = if unread == 0 {
        String::new()
    } else {
        format!(", {unread} not read")
    };
    format!("- Checks now: {passed} passed, {failed} failed, {running} running{unread}.")
}

/// The first [`LISTED_ITEMS`] of `items` as sub-items, and how many more.
fn listed<T>(items: &[T], line: fn(&T) -> String) -> Vec<String> {
    let mut lines: Vec<String> = items.iter().take(LISTED_ITEMS).map(line).collect();
    if items.len() > LISTED_ITEMS {
        lines.push(format!("  - and {} more", items.len() - LISTED_ITEMS));
    }
    lines
}

fn check_line(check: &CheckRef) -> String {
    let how = match check.failure {
        Failure::Failed => "",
        Failure::Cancelled => " (cancelled)",
    };
    let url = check
        .url
        .as_deref()
        .map(|url| format!(" {url}"))
        .unwrap_or_default();
    format!("  - {}{how}{url}", check.name)
}

/// `alice (MEMBER) on src/lib.rs: "quote" url`: who, their standing in the
/// repository, where, what a review decided, and what they wrote.
fn remark_line(remark: &Remark) -> String {
    let association = remark
        .association
        .as_deref()
        .map(|association| format!(" ({association})"))
        .unwrap_or_default();
    let place = remark
        .path
        .as_deref()
        .map(|path| format!(" on {path}"))
        .unwrap_or_default();
    let verdict = match remark.verdict {
        None | Some(Reviewed::Commented) => "",
        Some(Reviewed::Approved) => " approved",
        Some(Reviewed::ChangesRequested) => " requested changes",
        Some(Reviewed::Dismissed) => " (review dismissed)",
    };
    let quote = snippet(&remark.body);
    let said = if quote.is_empty() {
        String::new()
    } else {
        format!(": \"{quote}\"")
    };
    format!(
        "  - {}{association}{place}{verdict}{said} {}",
        remark.author, remark.url
    )
}

fn end_line(end: &End, label: &str, url: &str) -> String {
    match end {
        End::Merged { by, commit, base } => {
            let by = by
                .as_deref()
                .map(|by| format!(" by {by}"))
                .unwrap_or_default();
            let commit = commit
                .as_deref()
                .map(|commit| format!(" as {}", short(commit)))
                .unwrap_or_default();
            format!(
                "Pull request {label} ({url}) was merged into {base}{by}{commit}, so rho stopped watching it."
            )
        }
        End::Closed => format!(
            "Pull request {label} ({url}) was closed without merging, so rho stopped watching it. If it is reopened, call watch_pull_request to watch it again."
        ),
        End::CommentLoop => format!(
            "rho stopped watching pull request {label} ({url}) after {COMMENT_WAKE_LIMIT} comment-only updates in a row (limit {COMMENT_WAKE_LIMIT}). Call watch_pull_request to watch it again."
        ),
        End::Unreadable(why) => format!(
            "rho stopped watching pull request {label} ({url}) after {READ_FAILURE_LIMIT} failed reads in a row (limit {READ_FAILURE_LIMIT}): {}. Check it yourself, then call watch_pull_request to watch it again.",
            why.trim_end_matches('.')
        ),
    }
}

fn change_summary(change: &Change) -> &'static str {
    match change {
        Change::ChecksFailed { .. } => "checks failed",
        Change::ChecksPassed { .. } => "checks passed",
        Change::ChecksFinished { .. } => "checks finished",
        Change::Remarks { .. } => "new comments",
        Change::Conflicting => "merge conflict",
    }
}

fn end_summary(end: &End) -> &'static str {
    match end {
        End::Merged { .. } => "merged",
        End::Closed => "closed",
        End::CommentLoop => "stopped watching (comment loop)",
        End::Unreadable(_) => "stopped watching (unreadable)",
    }
}

fn change_tone(change: &Change) -> NotificationTone {
    match change {
        Change::ChecksFailed { .. } | Change::Conflicting => NotificationTone::Error,
        Change::ChecksFinished { failing, .. } if !failing.is_empty() => NotificationTone::Error,
        Change::ChecksFinished { .. } | Change::ChecksPassed { .. } => NotificationTone::Success,
        Change::Remarks { .. } => NotificationTone::Accent,
    }
}

fn end_tone(end: &End) -> NotificationTone {
    match end {
        End::Merged { .. } => NotificationTone::Success,
        End::Closed | End::CommentLoop | End::Unreadable(_) => NotificationTone::Warning,
    }
}

/// Which of a card's tones wins: the one that most needs a look.
fn urgency(tone: NotificationTone) -> u8 {
    match tone {
        NotificationTone::Error => 4,
        NotificationTone::Warning => 3,
        NotificationTone::Accent => 2,
        NotificationTone::Success => 1,
        // Neutral, and tones this crate doesn't produce.
        _ => 0,
    }
}

/// A comment's opening for the model: HTML comments and tags dropped from
/// its prose (GitHub renders them, so they are presentation, not what was
/// said), Markdown links, quote markers, headings and bold markers reduced
/// to their text so they don't spend the cut, code kept as written,
/// whitespace collapsed, and cut to [`SNIPPET_CHARS`]. After t3's
/// `snippet` (`pullRequestWatch.ts`).
fn snippet(body: &str) -> String {
    let text = without_markup(&without_html_comments(body));
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= SNIPPET_CHARS {
        return text;
    }
    let cut: String = text.chars().take(SNIPPET_CHARS - 3).collect();
    format!("{cut}...")
}

/// `body` with each closed `<!-- ... -->` replaced by a space.
fn without_html_comments(body: &str) -> String {
    let mut text = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<!--") {
        let Some(end) = rest[start + 4..].find("-->") else {
            break;
        };
        text.push_str(&rest[..start]);
        text.push(' ');
        rest = &rest[start + 4 + end + 3..];
    }
    text.push_str(rest);
    text
}

/// `body` with HTML tags dropped from its Markdown prose. Fenced code
/// blocks, and indented ones after a blank line, pass through untouched.
fn without_markup(body: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut fence: Option<&str> = None;
    let mut after_text = false;
    for line in body.split('\n') {
        let code = if let Some(marker) = fence {
            let closer = line.trim();
            let mark = marker.chars().next();
            if closer.len() >= marker.len() && closer.chars().all(|ch| Some(ch) == mark) {
                fence = None;
            }
            true
        } else if let Some(marker) = fence_opener(line) {
            fence = Some(marker);
            true
        } else {
            // After text, an indent continues the paragraph.
            !after_text && is_indented_code(line)
        };
        if code {
            if !prose.is_empty() {
                parts.push(plain_prose(&without_tags(&prose.join("\n"))));
                prose.clear();
            }
            parts.push(line.to_owned());
            after_text = false;
        } else {
            prose.push(line);
            after_text = !line.trim().is_empty();
        }
    }
    if !prose.is_empty() {
        parts.push(plain_prose(&without_tags(&prose.join("\n"))));
    }
    parts.join("\n")
}

/// The fence a line opens a code block with: three or more backticks or
/// tildes after at most three spaces. A backtick fence's info string has no
/// backtick, or it is inline code.
fn fence_opener(line: &str) -> Option<&str> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let mark = rest.chars().next().filter(|ch| matches!(ch, '`' | '~'))?;
    let run = rest.len() - rest.trim_start_matches(mark).len();
    let (marker, info) = rest.split_at(run);
    (run >= 3 && !(mark == '`' && info.contains('`'))).then_some(marker)
}

/// Whether a line is indented code: four spaces, or a tab after at most
/// three, before some text.
fn is_indented_code(line: &str) -> bool {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    let indented = spaces >= 4 || line[spaces..].starts_with('\t');
    indented && !line.trim().is_empty()
}

/// HTML elements GitHub renders in a comment, by name. Lowercase only:
/// `Promise<T>` is a type argument.
const TAG_NAMES: &[&str] = &[
    "a",
    "abbr",
    "b",
    "blockquote",
    "br",
    "caption",
    "cite",
    "code",
    "dd",
    "del",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "ins",
    "kbd",
    "li",
    "mark",
    "ol",
    "p",
    "picture",
    "pre",
    "q",
    "s",
    "samp",
    "small",
    "source",
    "span",
    "strike",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "ul",
    "var",
];

/// `prose` with its HTML tags replaced by spaces. Code spans stay as
/// written. An opener glued to an identifier is a type argument
/// (`Vec<code>`), so it and its closer stay too.
fn without_tags(prose: &str) -> String {
    let bytes = prose.as_bytes();
    let mut text = String::with_capacity(prose.len());
    let mut kept: Vec<&str> = Vec::new();
    let (mut from, mut at) = (0, 0);
    while at < bytes.len() {
        match bytes[at] {
            b'`' => {
                let ticks = run_of(bytes, at, b'`');
                at = code_span_end(bytes, at + ticks, ticks).unwrap_or(at + ticks);
            }
            b'<' => {
                let Some(tag) = tag_at(&prose[at..]) else {
                    at += 1;
                    continue;
                };
                let end = at + tag.len;
                let glued = at.checked_sub(1).is_some_and(|before| {
                    bytes[before].is_ascii_alphanumeric() || bytes[before] == b'_'
                });
                let keep = if tag.closing {
                    let opener = kept.iter().position(|name| *name == tag.name);
                    opener.map(|opener| kept.swap_remove(opener)).is_some()
                } else if glued {
                    kept.push(tag.name);
                    true
                } else {
                    false
                };
                if !keep {
                    text.push_str(&prose[from..at]);
                    text.push(' ');
                    from = end;
                }
                at = end;
            }
            _ => at += 1,
        }
    }
    text.push_str(&prose[from..]);
    text
}

/// Markdown `prose` as its words: each line without its `>` quote markers
/// and `#` heading marks, `**` and `__` dropped, and each `[text](target)`
/// link or `![alt](target)` image as its text.
fn plain_prose(prose: &str) -> String {
    prose
        .split('\n')
        .map(|line| {
            let line = line.trim_start_matches(|ch: char| ch == '>' || ch.is_whitespace());
            let line = line.trim_start_matches('#').trim_start();
            without_link_targets(&line.replace("**", "").replace("__", ""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` with each `[label](target)` as `label`, and an image's `!` too.
/// Brackets not followed by a closed `(target)` on the line stay.
fn without_link_targets(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let link = rest[open + 1..].find("](").and_then(|label_end| {
            let label = &rest[open + 1..open + 1 + label_end];
            let target = &rest[open + 1 + label_end + 2..];
            let target_end = target.find(')')?;
            (!label.contains('[') && !target[..target_end].contains(char::is_whitespace))
                .then_some((label, open + 1 + label_end + 2 + target_end + 1))
        });
        match link {
            Some((label, end)) => {
                out.push_str(rest[..open].strip_suffix('!').unwrap_or(&rest[..open]));
                out.push_str(label);
                rest = &rest[end..];
            }
            None => {
                out.push_str(&rest[..=open]);
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// How many `byte`s in a row start at `at`.
fn run_of(bytes: &[u8], at: usize, byte: u8) -> usize {
    bytes[at..].iter().take_while(|&&next| next == byte).count()
}

/// Where a code span opened by `ticks` backticks ends, searching from
/// `from`: after the next run of exactly as many, before any blank line.
fn code_span_end(bytes: &[u8], from: usize, ticks: usize) -> Option<usize> {
    let mut at = from;
    while at < bytes.len() {
        match bytes[at] {
            b'`' => {
                let run = run_of(bytes, at, b'`');
                if run == ticks {
                    return Some(at + run);
                }
                at += run;
            }
            b'\n' => {
                let blank = bytes[at + 1..]
                    .iter()
                    .find(|&&next| !matches!(next, b' ' | b'\t' | b'\r'))
                    .is_some_and(|&next| next == b'\n');
                if blank {
                    return None;
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    None
}

/// An HTML tag at the start of `text`.
struct Tag {
    name: &'static str,
    closing: bool,
    /// Its length in bytes, through its `>`.
    len: usize,
}

/// The tag `text` starts with, if it names one of [`TAG_NAMES`] and closes
/// before the next `<`, quoted attribute values aside.
fn tag_at(text: &str) -> Option<Tag> {
    let after = text.strip_prefix('<')?;
    let closing = after.starts_with('/');
    let named = if closing { &after[1..] } else { after };
    let name_len = named.bytes().take_while(u8::is_ascii_alphanumeric).count();
    let name = TAG_NAMES
        .iter()
        .copied()
        .find(|known| *known == &named[..name_len])?;
    let rest = &named[name_len..];
    if !rest.starts_with(|ch: char| ch.is_whitespace() || ch == '/' || ch == '>') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut at = 0;
    loop {
        match *bytes.get(at)? {
            b'>' => break,
            b'<' => return None,
            quote @ (b'"' | b'\'') => {
                let close = rest[at + 1..].find(char::from(quote))?;
                at += close + 2;
            }
            _ => at += 1,
        }
    }
    Some(Tag {
        name,
        closing,
        len: text.len() - rest.len() + at + 1,
    })
}

#[cfg(test)]
#[path = "news_tests.rs"]
mod tests;
