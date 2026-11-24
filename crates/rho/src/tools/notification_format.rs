//! Shared budgeted assembly for background-completion notifications.

use rho_sdk::{floor_char_boundary, TRUNCATION_MARKER};

/// The most one notification's whole text may take.
pub(crate) const MODEL_NOTIFICATION_BYTES: usize = 16 * 1024;
/// The most of one result a notification quotes.
pub(crate) const RESULT_EXCERPT_BYTES: usize = 16 * 1024;

/// A result a notification quotes: `text` between `label` and `close`.
pub(crate) struct Excerpt<'a> {
    pub(crate) label: String,
    pub(crate) text: &'a str,
    pub(crate) close: String,
}

/// One notification within [`MODEL_NOTIFICATION_BYTES`]: `header`, the
/// `summaries`, one after another until the budget is spent and then a line
/// `omit` makes of how many were left out, then, after `heading`, each of
/// `excerpts`, its text cut to [`RESULT_EXCERPT_BYTES`] and to the room
/// left, then `footer`. Summaries take priority, so a large result cannot
/// hide the state of a later one.
pub(crate) fn bounded_notification(
    header: &str,
    footer: &str,
    summaries: impl IntoIterator<Item = String>,
    omit: impl Fn(usize) -> String,
    heading: &str,
    excerpts: Vec<Excerpt<'_>>,
) -> String {
    let body_bytes = MODEL_NOTIFICATION_BYTES.saturating_sub(header.len() + footer.len());
    let mut body = join_budgeted_sections(summaries, "\n", body_bytes, omit);
    if !excerpts.is_empty() {
        if body.len() + heading.len() > body_bytes {
            return format!("{header}{body}{footer}");
        }
        body.push_str(heading);
    }
    for Excerpt { label, text, close } in excerpts {
        if body.len() + label.len() + close.len() >= body_bytes {
            break;
        }
        body.push_str(&label);
        let available = (body_bytes - body.len() - close.len()).min(RESULT_EXCERPT_BYTES);
        push_excerpt(&mut body, text, available);
        body.push_str(&close);
    }
    format!("{header}{body}{footer}")
}

/// Appends `result` to `output`, cut with [`TRUNCATION_MARKER`] to at most
/// `max_bytes`.
fn push_excerpt(output: &mut String, result: &str, max_bytes: usize) {
    if result.len() <= max_bytes {
        output.push_str(result);
        return;
    }

    let prefix_bytes = max_bytes.saturating_sub(TRUNCATION_MARKER.len());
    let boundary = floor_char_boundary(result, prefix_bytes);
    output.push_str(&result[..boundary]);
    if max_bytes >= TRUNCATION_MARKER.len() {
        output.push_str(TRUNCATION_MARKER);
    }
}

/// Joins status sections until `budget` is exhausted, then an omission line
/// if that still fits.
pub(crate) fn join_budgeted_sections<I, S>(
    sections: I,
    separator: &str,
    budget: usize,
    omit: impl Fn(usize) -> String,
) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let sections: Vec<S> = sections.into_iter().collect();
    let mut body = String::new();
    for (index, section) in sections.iter().enumerate() {
        let section = section.as_ref();
        let separator = if index == 0 { "" } else { separator };
        let remaining = sections.len() - index;
        let omission = {
            let text = omit(remaining);
            if body.is_empty() {
                text
            } else {
                format!("{separator}{text}")
            }
        };
        // Reserve the omission marker so truncation is never silent.
        if body.len() + separator.len() + section.len() + omission.len() > budget {
            if body.len() + omission.len() <= budget {
                body.push_str(&omission);
            }
            break;
        }
        body.push_str(separator);
        body.push_str(section);
    }
    body
}
