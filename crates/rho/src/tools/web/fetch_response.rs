use super::{output, storage::StoredItem};

pub(super) const FETCH_CONTENT_TOOL: &str = "fetch_content";

/// Builds agent-facing fetch_content text.
///
/// Single targets inline as much body text as fits `max_output_bytes`. Multi-target
/// calls keep short selectors and point at `get_search_content` with urlIndex.
pub(super) fn build_fetch_content_output(
    response_id: &str,
    items: &[StoredItem],
    max_output_bytes: usize,
) -> String {
    match items {
        [item] => output::format_single_fetch(response_id, item, max_output_bytes),
        items => output::format_multi_fetch(response_id, items, max_output_bytes),
    }
}

#[cfg(test)]
#[path = "fetch_response_tests.rs"]
mod tests;
